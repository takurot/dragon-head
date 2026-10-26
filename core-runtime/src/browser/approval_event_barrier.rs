use super::*;
use headless_chrome::{
    browser::tab::EventListener,
    protocol::cdp::{types::Event, Page, Runtime},
};
use std::sync::{mpsc, Weak};

#[cfg(test)]
type BeforeMarkerHook = Arc<dyn Fn(&DocumentSnapshot) + Send + Sync>;

#[derive(Default)]
pub(super) struct ApprovalEventBarrier {
    world: Mutex<Option<IsolatedWorld>>,
    #[cfg(test)]
    before_marker: Mutex<Option<BeforeMarkerHook>>,
}

struct IsolatedWorld {
    frame_id: String,
    loader_id: String,
    context_id: Runtime::ExecutionContextId,
}

struct BarrierListener<'a> {
    tab: &'a headless_chrome::Tab,
    listener: Weak<dyn EventListener<Event> + Send + Sync>,
}

impl Drop for BarrierListener<'_> {
    fn drop(&mut self) {
        let _ = self.tab.remove_event_listener(&self.listener);
    }
}

impl ApprovalEventBarrier {
    fn context_id(
        &self,
        tab: &headless_chrome::Tab,
        document: &DocumentSnapshot,
    ) -> Result<Runtime::ExecutionContextId> {
        let mut world = self
            .world
            .lock()
            .map_err(|_| anyhow::anyhow!("Policy approval event barrier is unavailable"))?;
        if let Some(cached) = world.as_ref() {
            if cached.frame_id == document.frame_id && cached.loader_id == document.loader_id {
                return Ok(cached.context_id);
            }
        }
        tab.enable_runtime()?;
        let context_id = tab
            .call_method(Page::CreateIsolatedWorld {
                frame_id: document.frame_id.clone(),
                world_name: Some("dragon-head-approval-observer".to_string()),
                grant_univeral_access: Some(false),
            })?
            .execution_context_id;
        *world = Some(IsolatedWorld {
            frame_id: document.frame_id.clone(),
            loader_id: document.loader_id.clone(),
            context_id,
        });
        Ok(context_id)
    }

    /// A CDP response alone does not drain headless_chrome's separate tab-event thread.
    /// Emit a nonce in an isolated world and wait for that same event thread to acknowledge
    /// it, after the navigation observer has handled all preceding Page events. No DOM or
    /// main-world globals are changed; the only console payload is a fresh random nonce.
    pub(super) fn drain(
        &self,
        tab: &headless_chrome::Tab,
        document: &DocumentSnapshot,
    ) -> Result<()> {
        let result = (|| {
            let context_id = self.context_id(tab, document)?;
            #[cfg(test)]
            {
                let hook = self.before_marker.lock().unwrap().clone();
                if let Some(hook) = hook {
                    hook(document);
                }
            }
            let nonce = uuid::Uuid::new_v4().to_string();
            let expected = nonce.clone();
            let (sender, receiver) = mpsc::channel();
            let listener = tab.add_event_listener(Arc::new(move |event: &Event| {
                if marker_matches(event, context_id, &expected) {
                    let _ = sender.send(());
                }
            }))?;
            let _listener = BarrierListener { tab, listener };
            let evaluated = tab.call_method(Runtime::Evaluate {
                expression: format!("console.debug({})", serde_json::to_string(&nonce)?),
                context_id: Some(context_id),
                silent: Some(false),
                return_by_value: Some(true),
                object_group: None,
                include_command_line_api: Some(false),
                generate_preview: Some(false),
                user_gesture: Some(false),
                await_promise: Some(false),
                throw_on_side_effect: None,
                timeout: Some(1_000.0),
                disable_breaks: None,
                repl_mode: None,
                allow_unsafe_eval_blocked_by_csp: None,
                unique_context_id: None,
                serialization_options: None,
            })?;
            if evaluated.exception_details.is_some() {
                anyhow::bail!("Policy approval event barrier evaluation failed");
            }
            receiver
                .recv_timeout(Duration::from_secs(5))
                .map_err(|_| anyhow::anyhow!("Policy approval event barrier timed out"))?;
            Ok(())
        })();
        if result.is_err() {
            if let Ok(mut world) = self.world.lock() {
                *world = None;
            }
        }
        // CDP failures may contain document URLs; expose only the fixed safe reason.
        result.map_err(|_: anyhow::Error| {
            anyhow::anyhow!("Could not synchronize the policy approval document")
        })
    }
}

fn marker_matches(event: &Event, context_id: Runtime::ExecutionContextId, nonce: &str) -> bool {
    matches!(event, Event::RuntimeConsoleAPICalled(event)
        if event.params.execution_context_id == context_id
            && event.params.args.len() == 1
            && event.params.args[0].value.as_ref().and_then(serde_json::Value::as_str) == Some(nonce))
}

impl PageSession {
    pub(super) fn approval_document_snapshot(&self) -> Result<DocumentSnapshot> {
        for _ in 0..3 {
            let document = self
                .live_document_snapshot()
                .map_err(|_| anyhow::anyhow!("Could not read the policy approval document"))?;
            self.approval_event_barrier.drain(&self.inner, &document)?;
            if self
                .with_approval_document_context_current(&document, || Ok(()))
                .is_ok()
            {
                return Ok(document);
            }
        }
        anyhow::bail!("Policy approval document changed during synchronization")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    mod fixture {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/tests/support/approval_fixture.rs"
        ));
    }

    fn marker(context_id: Runtime::ExecutionContextId, nonce: &str) -> Event {
        serde_json::from_value(serde_json::json!({
            "method": "Runtime.consoleAPICalled",
            "params": {"type": "debug", "args": [{"type": "string", "value": nonce}],
                       "executionContextId": context_id, "timestamp": 0},
        }))
        .unwrap()
    }

    #[test]
    fn marker_requires_the_exact_nonce_and_isolated_execution_context() {
        assert!(marker_matches(&marker(1, "expected"), 1, "expected"));
        assert!(!marker_matches(&marker(1, "wrong"), 1, "expected"));
        assert!(!marker_matches(&marker(2, "expected"), 1, "expected"));
    }

    #[test]
    fn security_snapshot_drains_a_delayed_same_document_a_b_a_event_queue() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let fixture = fixture::Fixture::new(false)?;
        let client = BrowserClient::new()?;
        let page = Arc::new(client.new_page()?);
        page.navigate(&fixture.url)?;
        let original = page.approval_document_snapshot()?;
        let pause_nonce = uuid::Uuid::new_v4().to_string();
        let expected = pause_nonce.clone();
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let release_rx = Mutex::new(release_rx);
        let listener = page
            .inner
            .add_event_listener(Arc::new(move |event: &Event| {
                if let Event::RuntimeConsoleAPICalled(event) = event {
                    if event.params.args.iter().any(|arg| {
                        arg.value.as_ref().and_then(serde_json::Value::as_str)
                            == Some(expected.as_str())
                    }) {
                        let _ = entered_tx.send(());
                        // Timeout bounds cleanup even when an assertion on the driving thread fails.
                        let _ = release_rx
                            .lock()
                            .unwrap()
                            .recv_timeout(Duration::from_secs(5));
                    }
                }
            }))?;
        let _listener = BarrierListener {
            tab: &page.inner,
            listener,
        };
        page.evaluate_script(&format!(
            "console.debug({})",
            serde_json::to_string(&pause_nonce)?
        ))?;
        entered_rx.recv_timeout(Duration::from_secs(5))?;
        page.evaluate_script(&format!(
            "history.replaceState(null, '', '/other'); history.replaceState(null, '', {})",
            serde_json::to_string(&fixture.url)?,
        ))?;
        // Responses are independent of the blocked event thread: the live URL is back at A
        // while its B/A observation events are still provably queued behind our channel gate.
        let stale = page.live_document_snapshot()?;
        assert_eq!(stale.url, original.url);
        assert_eq!(stale.revision, original.revision);
        let (captured_tx, captured_rx) = mpsc::channel();
        let (continue_tx, continue_rx) = mpsc::channel();
        let continue_rx = Mutex::new(continue_rx);
        let captured = AtomicBool::new(false);
        *page.approval_event_barrier.before_marker.lock().unwrap() =
            Some(Arc::new(move |snapshot| {
                if !captured.swap(true, Ordering::Relaxed) {
                    let _ = captured_tx.send(snapshot.clone());
                    let _ = continue_rx
                        .lock()
                        .unwrap()
                        .recv_timeout(Duration::from_secs(5));
                }
            }));
        let snapshot_page = Arc::clone(&page);
        let snapshot_thread = thread::spawn(move || snapshot_page.approval_document_snapshot());
        // The worker must capture the old document and ENTER its actual event barrier while
        // the event thread is still blocked. Removing the drain call makes this handshake fail.
        let captured = captured_rx.recv_timeout(Duration::from_secs(5))?;
        assert_eq!(captured.revision, original.revision);
        assert_eq!(captured.url, original.url);
        continue_tx.send(())?;
        release_tx.send(())?;
        let fresh = snapshot_thread.join().expect("snapshot worker panicked")?;
        assert_eq!(fresh.url, original.url);
        assert!(fresh.revision > original.revision);
        Ok(())
    }

    #[test]
    fn marker_timeout_fails_closed_and_scoped_listeners_are_removed() -> Result<()> {
        if test_bench_support::should_skip_browser_tests() {
            return Ok(());
        }
        let client = BrowserClient::new()?;
        let page = client.new_page()?;
        page.navigate("data:text/html,private-content")?;
        let document = page.approval_document_snapshot()?;
        let context_id = page
            .approval_event_barrier
            .context_id(&page.inner, &document)?;
        // This isolated-world change is a deliberate test fault. Page globals are untouched.
        let mut command: Runtime::Evaluate = serde_json::from_value(serde_json::json!({
            "expression": "console.debug = () => {}", "contextId": context_id,
        }))?;
        command.return_by_value = Some(true);
        page.inner.call_method(command)?;
        let error = page
            .approval_event_barrier
            .drain(&page.inner, &document)
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "Could not synchronize the policy approval document"
        );
        assert!(!error.to_string().contains("private-content"));
        assert!(page.approval_event_barrier.world.lock().unwrap().is_none());
        let listener = page.inner.add_event_listener(Arc::new(|_: &Event| {}))?;
        let weak = listener.clone();
        {
            let _listener = BarrierListener {
                tab: &page.inner,
                listener,
            };
        }
        assert!(
            weak.upgrade().is_none(),
            "scoped listener must not remain installed"
        );
        Ok(())
    }
}
