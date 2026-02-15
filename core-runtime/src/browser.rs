use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use crate::{
    error::{ActionError, VerifyError, WaitError},
    sre::{normalize_dom, LoadProfile, SemanticNode, SemanticState},
};

const DEFAULT_WAIT_POLL_INTERVAL: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SemanticTarget {
    Id(i64),
    StableKey(String),
    IdWithStableKey { id: i64, stable_key: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SemanticWaitState {
    Enabled,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SemanticWaitOptions {
    pub load_profile: LoadProfile,
    pub poll_interval: Duration,
}

impl Default for SemanticWaitOptions {
    fn default() -> Self {
        Self {
            load_profile: LoadProfile::Minimal,
            poll_interval: DEFAULT_WAIT_POLL_INTERVAL,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SomTrigger {
    GetVisual,
    ActAmbiguous,
    VerifyFailed,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SomMark {
    pub id: i64,
    pub stable_key: Option<String>,
    /// `[x, y, width, height]` in CSS pixels.
    pub bbox: [f64; 4],
}

#[derive(Debug, Clone, PartialEq)]
pub struct VisualCapture {
    pub trigger: SomTrigger,
    pub marks: Vec<SomMark>,
    pub image_png: Vec<u8>,
}

#[derive(Default)]
struct SomPipelineState {
    generation_count: usize,
    last_capture: Option<VisualCapture>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct StableKeyIndexEntry {
    backend_node_id: i64,
    alias: Option<String>,
}

pub struct BrowserClient {
    inner: Browser,
}

impl BrowserClient {
    pub fn new() -> Result<Self> {
        let options = LaunchOptions::default_builder()
            .headless(true)
            .build()
            .context("Failed to build launch options")?;

        let browser = Browser::new(options).context("Failed to launch browser")?;
        Ok(Self { inner: browser })
    }

    pub fn new_page(&self) -> Result<PageSession> {
        let tab = self.inner.new_tab().context("Failed to create new tab")?;
        Ok(PageSession {
            inner: tab,
            som_pipeline: Arc::new(Mutex::new(SomPipelineState::default())),
            stable_key_index: Arc::new(Mutex::new(HashMap::new())),
        })
    }
}

pub struct PageSession {
    inner: Arc<headless_chrome::Tab>,
    som_pipeline: Arc<Mutex<SomPipelineState>>,
    stable_key_index: Arc<Mutex<HashMap<String, StableKeyIndexEntry>>>,
}

impl PageSession {
    pub fn navigate(&self, url: &str) -> Result<()> {
        self.inner.navigate_to(url).context("Failed to navigate")?;
        self.inner
            .wait_until_navigated()
            .context("Failed to wait for navigation")?;
        self.clear_stable_key_index();
        Ok(())
    }

    pub fn get_content(&self) -> Result<String> {
        self.inner
            .get_content()
            .context("Failed to get page content")
    }

    pub fn get_title(&self) -> Result<String> {
        self.inner.get_title().context("Failed to get page title")
    }

    pub fn get_document_node(&self) -> Result<headless_chrome::protocol::cdp::DOM::Node> {
        // Enforce DOM domain enablement if not already?
        // Tab usually enables domains on demand or we might need to do it.
        // But let's try calling get_document.
        let root = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetDocument {
                depth: Some(1000), // Retrieve full depth? Or default? Default is usually deep?
                // spec says: "The maximum depth at which children should be retrieved, defaults to 1. Use -1 for the entire subtree".
                // We need full tree for SRE.
                // Using 1000 as a large enough depth since -1 (full) is not supported by headless_chrome u32 type.
                pierce: Some(true), // Traverse iframes? SPEC doesn't specify, but safer for full context.
            })?;
        Ok(root.root)
    }

    pub fn evaluate_script(
        &self,
        script: &str,
    ) -> Result<headless_chrome::protocol::cdp::Runtime::RemoteObject> {
        self.inner
            .evaluate(script, false)
            .context("Failed to evaluate script")
    }

    fn evaluate_script_value(
        &self,
        script: &str,
        await_promise: bool,
    ) -> Result<serde_json::Value> {
        use headless_chrome::protocol::cdp::Runtime::Evaluate;

        let result = self
            .inner
            .call_method(Evaluate {
                expression: script.to_string(),
                return_by_value: Some(true),
                generate_preview: Some(false),
                silent: Some(true),
                await_promise: Some(await_promise),
                include_command_line_api: Some(false),
                user_gesture: Some(false),
                object_group: None,
                context_id: None,
                throw_on_side_effect: None,
                timeout: None,
                disable_breaks: None,
                repl_mode: None,
                allow_unsafe_eval_blocked_by_csp: None,
                unique_context_id: None,
                serialization_options: None,
            })
            .context("Failed to evaluate script value")?;

        result
            .result
            .value
            .context("Script evaluation did not return a value")
    }

    fn ensure_sre_event_bridge(&self) -> Result<u64> {
        let script = r#"
(() => {
    const existing = window.__nrSreEventBridge;
    if (existing) {
        return existing.version;
    }

    const state = { version: 0, waiters: [] };
    const notify = () => {
        state.version += 1;
        const waiters = state.waiters.splice(0, state.waiters.length);
        for (const waiter of waiters) {
            try {
                waiter(state.version);
            } catch (_ignored) {}
        }
    };

    const target = document.documentElement || document;
    const observer = new MutationObserver(() => notify());
    observer.observe(target, {
        subtree: true,
        childList: true,
        attributes: true,
        characterData: true,
    });

    state.observer = observer;
    window.__nrSreEventBridge = state;
    return state.version;
})()
"#;

        let value = self
            .evaluate_script_value(script, false)
            .context("Failed to initialize SRE event bridge")?;
        value_to_u64(&value).context("Invalid SRE event bridge version")
    }

    fn wait_for_sre_event(&self, last_seen_version: u64, timeout: Duration) -> Result<u64> {
        let timeout_ms = duration_to_millis(timeout);
        let script = format!(
            r#"
(() => {{
    const state = window.__nrSreEventBridge;
    if (!state) {{
        return Promise.resolve({last_seen_version});
    }}

    if (state.version > {last_seen_version}) {{
        return Promise.resolve(state.version);
    }}

    return new Promise((resolve) => {{
        let settled = false;
        const complete = (version) => {{
            if (settled) {{
                return;
            }}
            settled = true;
            clearTimeout(timer_id);
            const index = state.waiters.indexOf(waiter);
            if (index >= 0) {{
                state.waiters.splice(index, 1);
            }}
            resolve(version);
        }};

        const waiter = (version) => complete(version);
        const timer_id = setTimeout(() => complete(state.version), {timeout_ms});
        state.waiters.push(waiter);
    }});
}})()
"#
        );

        let value = self
            .evaluate_script_value(&script, true)
            .context("Failed while waiting for SRE event")?;
        value_to_u64(&value).context("Invalid SRE event version")
    }

    /// Explicitly request a visual capture with SoM marks.
    pub fn get_visual(&self) -> Result<VisualCapture> {
        self.capture_som(SomTrigger::GetVisual)
    }

    /// Returns the number of SoM captures generated for this page session.
    pub fn som_generation_count(&self) -> usize {
        self.som_pipeline
            .lock()
            .map(|state| state.generation_count)
            .unwrap_or_default()
    }

    /// Returns the latest SoM capture, if any.
    pub fn last_visual_capture(&self) -> Option<VisualCapture> {
        self.som_pipeline
            .lock()
            .ok()
            .and_then(|state| state.last_capture.clone())
    }

    /// Refresh the per-session stable_key index from the latest semantic capture.
    /// Returns the number of indexed nodes.
    pub fn refresh_stable_key_index(&self, profile: LoadProfile) -> Result<usize> {
        self.capture_state(profile)?;
        Ok(self
            .stable_key_index
            .lock()
            .map(|index| index.len())
            .unwrap_or_default())
    }

    /// Resolve a backend node id from the per-session stable_key index.
    pub fn lookup_backend_node_id_by_stable_key(&self, stable_key: &str) -> Option<i64> {
        self.stable_key_index
            .lock()
            .ok()
            .and_then(|index| index.get(stable_key).map(|entry| entry.backend_node_id))
    }

    /// Resolve alias metadata from the per-session stable_key index.
    pub fn lookup_alias_by_stable_key(&self, stable_key: &str) -> Option<String> {
        self.stable_key_index
            .lock()
            .ok()
            .and_then(|index| index.get(stable_key).and_then(|entry| entry.alias.clone()))
    }

    /// Verify element text against an expected value.
    /// On mismatch, triggers a SoM capture to help disambiguate recovery.
    pub fn verify_text(&self, target_id: i64, expected_text: &str) -> Result<()> {
        let actual_text = self.get_element_text(target_id)?;
        if normalize_text(&actual_text) == normalize_text(expected_text) {
            return Ok(());
        }

        self.trigger_som_capture_best_effort(SomTrigger::VerifyFailed);
        Err(VerifyError::ExpectationMismatch {
            target_id,
            expected: expected_text.to_string(),
            actual: actual_text,
        }
        .into())
    }

    /// Perform an action on a target element.
    /// Uses `target_id` (backend_node_id) preferentially.
    /// If `target_id` is invalid (stale), attempts fallback using `stable_key` by re-scanning the DOM.
    pub fn act(
        &self,
        target_id: Option<i64>,
        stable_key: Option<&str>,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // First attempt: use target_id if available
        if let Some(bid) = target_id {
            match self.perform_action_by_id(bid, action, value) {
                Ok(_) => return Ok(()),
                Err(e) => {
                    // Only fallback if the error indicates a node issue (e.g. "Could not find node", "No node with given id")
                    // If it's a timeout or other error, we probably shouldn't blindly retry?
                    // The CDP error for invalid backend_node_id usually says "Could not find node with given id".
                    let node_error_markers = [
                        "could not find node",
                        "no node with given id",
                        "could not find object with given id",
                        "no object with given id",
                        "node does not exist",
                    ];
                    let is_node_error = error_chain_contains_any(&e, &node_error_markers);

                    if !is_node_error {
                        return Err(e);
                    }

                    if stable_key.is_none() {
                        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
                        return Err(ActionError::VerifyRequired.into());
                    }
                    // Fallback proceed...
                }
            }
        }

        // Fallback: use stable_key
        if let Some(key) = stable_key {
            // Refresh semantic snapshot and stable-key index before lookup.
            self.capture_state(crate::sre::LoadProfile::Interactive)?;

            if let Some(new_id) = self.lookup_backend_node_id_by_stable_key(key) {
                // Log success of fallback
                eprintln!(
                    "[WARN] Action recovered via stable key: {} -> new_id: {}",
                    key, new_id
                );
                return self.perform_action_by_id(new_id, action, value);
            } else {
                // Both failures -> VerifyRequired
                self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
                return Err(ActionError::VerifyRequired.into());
            }
        }

        // If we reach here, we had no stable key or it failed lookup, and target_id failed or wasn't provided.
        // Actually, if stable_key was None, we would have returned early in the target_id block if target_id was Some.
        // If target_id was None AND stable_key was None, we should also error.

        self.trigger_som_capture_best_effort(SomTrigger::ActAmbiguous);
        Err(ActionError::VerifyRequired.into())
    }

    /// Wait until a semantic target reaches the requested state.
    pub fn wait_for_semantic(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
    ) -> Result<()> {
        self.wait_for_semantic_with_options(
            target,
            desired_state,
            timeout,
            SemanticWaitOptions::default(),
        )
    }

    /// Wait until a semantic target reaches the requested state with explicit options.
    pub fn wait_for_semantic_with_options(
        &self,
        target: SemanticTarget,
        desired_state: SemanticWaitState,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile)?;
        let started = Instant::now();

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!(
                        "semantic target {:?} to become {:?}",
                        target, desired_state
                    ),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if target_matches_state(&state, &target, desired_state) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for semantic target state"));
                    }
                }
            }
        }
    }

    /// Wait until the specified intent marker appears in semantic state updates.
    pub fn wait_for_intent(&self, intent: &str, timeout: Duration) -> Result<()> {
        self.wait_for_intent_with_options(intent, timeout, SemanticWaitOptions::default())
    }

    /// Wait until the specified intent marker appears with explicit options.
    pub fn wait_for_intent_with_options(
        &self,
        intent: &str,
        timeout: Duration,
        options: SemanticWaitOptions,
    ) -> Result<()> {
        let mut subscriber = SreEventSubscriber::new(self, options.load_profile)?;
        let started = Instant::now();

        loop {
            let remaining = remaining_timeout(started, timeout);
            if remaining.is_zero() {
                return Err(WaitError::Timeout {
                    operation: format!("intent '{}'", intent),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            match subscriber.wait_next_state_event(remaining) {
                Ok(Some(state)) => {
                    if state_contains_intent(state.root(), intent) {
                        return Ok(());
                    }
                }
                Ok(None) => {}
                Err(err) => {
                    if !is_transient_capture_error(&err) {
                        return Err(err.context("Failed while waiting for intent"));
                    }
                }
            }
        }
    }

    fn capture_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        let root = self.get_document_node()?;
        let sem_root = normalize_dom(profile, &root)?;
        self.replace_stable_key_index(&sem_root);
        Ok(SemanticState::new(sem_root, profile))
    }

    fn replace_stable_key_index(&self, root: &SemanticNode) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
            collect_stable_key_entries(root, &mut index);
        }
    }

    fn clear_stable_key_index(&self) {
        if let Ok(mut index) = self.stable_key_index.lock() {
            index.clear();
        }
    }

    fn capture_som(&self, trigger: SomTrigger) -> Result<VisualCapture> {
        let root = self.get_document_node()?;
        let semantic_root = normalize_dom(LoadProfile::Visual, &root)?;

        let mut marks = Vec::new();
        self.collect_som_marks(&semantic_root, &mut marks);

        let image_png = self
            .inner
            .capture_screenshot(
                headless_chrome::protocol::cdp::Page::CaptureScreenshotFormatOption::Png,
                None,
                None,
                true,
            )
            .context("Failed to capture SoM screenshot")?;

        let capture = VisualCapture {
            trigger,
            marks,
            image_png,
        };
        self.store_som_capture(capture.clone());
        Ok(capture)
    }

    fn trigger_som_capture_best_effort(&self, trigger: SomTrigger) {
        if let Err(err) = self.capture_som(trigger) {
            eprintln!("[WARN] SoM capture trigger failed ({trigger:?}): {err:#}");
        }
    }

    fn store_som_capture(&self, capture: VisualCapture) {
        if let Ok(mut state) = self.som_pipeline.lock() {
            state.generation_count += 1;
            state.last_capture = Some(capture);
        }
    }

    fn collect_som_marks(&self, node: &SemanticNode, out: &mut Vec<SomMark>) {
        if node.backend_node_id > 0 && node.stable_key.is_some() {
            if let Ok(Some(bbox)) = self.resolve_node_bbox(node.backend_node_id) {
                out.push(SomMark {
                    id: node.backend_node_id,
                    stable_key: node.stable_key.clone(),
                    bbox,
                });
            }
        }

        for child in &node.children {
            self.collect_som_marks(child, out);
        }
    }

    fn resolve_node_bbox(&self, backend_node_id: i64) -> Result<Option<[f64; 4]>> {
        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let model = self
            .inner
            .call_method(headless_chrome::protocol::cdp::DOM::GetBoxModel {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_id: None,
            })
            .context("Failed to get box model")?
            .model;

        Ok(quad_to_bbox(&model.content))
    }

    fn get_element_text(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;

        let object_id = self.resolve_node_object_id(backend_node_id)?;
        let result = self
            .inner
            .call_method(CallFunctionOn {
                object_id: Some(object_id),
                function_declaration:
                    "function() { return (this.innerText || this.textContent || '').trim(); }"
                        .to_string(),
                arguments: None,
                silent: Some(true),
                return_by_value: Some(true),
                generate_preview: Some(false),
                user_gesture: Some(false),
                await_promise: Some(false),
                execution_context_id: None,
                object_group: None,
                throw_on_side_effect: None,
                unique_context_id: None,
                serialization_options: None,
            })
            .context("Failed to extract element text")?;

        let value = result
            .result
            .value
            .unwrap_or_else(|| serde_json::Value::String(String::new()));

        if let Some(text) = value.as_str() {
            Ok(text.to_string())
        } else {
            Ok(value.to_string())
        }
    }

    fn resolve_node_object_id(&self, backend_node_id: i64) -> Result<String> {
        use headless_chrome::protocol::cdp::DOM::ResolveNode;

        let node_id_u32 =
            u32::try_from(backend_node_id).context("Invalid backend_node_id: must fit in u32")?;

        let remote_object = self
            .inner
            .call_method(ResolveNode {
                node_id: None,
                backend_node_id: Some(node_id_u32),
                object_group: None,
                execution_context_id: None,
            })
            .context("Failed to resolve node")?
            .object;

        remote_object
            .object_id
            .context("Failed to resolve node to object")
    }

    fn perform_action_by_id(
        &self,
        backend_node_id: i64,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // Resolve backend_node_id to RemoteObject
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;
        let object_id = self.resolve_node_object_id(backend_node_id)?;

        match action {
            "click" => {
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id),
                    function_declaration: "function() { this.click(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
            }
            "type" => {
                let text = value.context("Value is required for type action")?;
                // Focus then type
                self.inner.call_method(CallFunctionOn {
                    object_id: Some(object_id.clone()),
                    function_declaration: "function() { this.focus(); }".to_string(),
                    arguments: None,
                    silent: Some(true),
                    return_by_value: Some(false),
                    generate_preview: Some(false),
                    user_gesture: Some(true),
                    await_promise: Some(false),
                    execution_context_id: None,
                    object_group: None,
                    throw_on_side_effect: None,
                    unique_context_id: None,
                    serialization_options: None,
                })?;
                self.inner.type_str(text)?;
            }
            _ => anyhow::bail!("Unsupported action: {}", action),
        }
        Ok(())
    }
}

struct SreEventSubscriber<'a> {
    session: &'a PageSession,
    profile: LoadProfile,
    last_state_hash: Option<String>,
    last_event_version: u64,
    initial_snapshot_emitted: bool,
}

impl<'a> SreEventSubscriber<'a> {
    fn new(session: &'a PageSession, profile: LoadProfile) -> Result<Self> {
        let last_event_version = session.ensure_sre_event_bridge()?;
        Ok(Self {
            session,
            profile,
            last_state_hash: None,
            last_event_version,
            initial_snapshot_emitted: false,
        })
    }

    fn wait_next_state_event(&mut self, max_wait: Duration) -> Result<Option<SemanticState>> {
        if !self.initial_snapshot_emitted {
            self.initial_snapshot_emitted = true;
            return self.capture_next_state_if_changed();
        }

        let bridge_version = self.session.ensure_sre_event_bridge()?;
        if bridge_version != self.last_event_version {
            self.last_event_version = bridge_version;
            return self.capture_next_state_if_changed();
        }

        let next_version = self
            .session
            .wait_for_sre_event(self.last_event_version, max_wait)?;
        if next_version == self.last_event_version {
            return Ok(None);
        }

        self.last_event_version = next_version;
        self.capture_next_state_if_changed()
    }

    fn capture_next_state_if_changed(&mut self) -> Result<Option<SemanticState>> {
        let state = self.session.capture_state(self.profile)?;
        let current_hash = state.state_hash().to_owned();

        if self.last_state_hash.as_deref() == Some(current_hash.as_str()) {
            return Ok(None);
        }

        self.last_state_hash = Some(current_hash);
        Ok(Some(state))
    }
}

fn target_matches_state(
    state: &SemanticState,
    target: &SemanticTarget,
    desired_state: SemanticWaitState,
) -> bool {
    let resolved = match target {
        SemanticTarget::Id(id) => find_node_by_id(state.root(), *id),
        SemanticTarget::StableKey(key) => find_node_by_key(state.root(), key),
        SemanticTarget::IdWithStableKey { id, stable_key } => find_node_by_id(state.root(), *id)
            .or_else(|| find_node_by_key(state.root(), stable_key)),
    };

    resolved.is_some_and(|node| node_matches_state(node, desired_state))
}

fn find_node_by_id(node: &SemanticNode, target_id: i64) -> Option<&SemanticNode> {
    if node.backend_node_id == target_id {
        return Some(node);
    }

    for child in &node.children {
        if let Some(found) = find_node_by_id(child, target_id) {
            return Some(found);
        }
    }

    None
}

fn find_node_by_key<'a>(node: &'a SemanticNode, target_key: &str) -> Option<&'a SemanticNode> {
    if let Some(key) = &node.stable_key {
        if key == target_key {
            return Some(node);
        }
    }

    for child in &node.children {
        if let Some(found) = find_node_by_key(child, target_key) {
            return Some(found);
        }
    }

    None
}

fn collect_stable_key_entries(node: &SemanticNode, out: &mut HashMap<String, StableKeyIndexEntry>) {
    if let Some(key) = &node.stable_key {
        if node.backend_node_id > 0 {
            out.insert(
                key.clone(),
                StableKeyIndexEntry {
                    backend_node_id: node.backend_node_id,
                    alias: node.alias.clone(),
                },
            );
        }
    }

    for child in &node.children {
        collect_stable_key_entries(child, out);
    }
}

fn quad_to_bbox(quad: &[f64]) -> Option<[f64; 4]> {
    if quad.len() < 8 {
        return None;
    }

    let xs = [quad[0], quad[2], quad[4], quad[6]];
    let ys = [quad[1], quad[3], quad[5], quad[7]];

    let min_x = xs.iter().copied().fold(f64::INFINITY, f64::min);
    let max_x = xs.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    let min_y = ys.iter().copied().fold(f64::INFINITY, f64::min);
    let max_y = ys.iter().copied().fold(f64::NEG_INFINITY, f64::max);

    if !min_x.is_finite() || !max_x.is_finite() || !min_y.is_finite() || !max_y.is_finite() {
        return None;
    }

    Some([
        min_x,
        min_y,
        (max_x - min_x).max(0.0),
        (max_y - min_y).max(0.0),
    ])
}

fn normalize_text(input: &str) -> String {
    input
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
}

fn node_matches_state(node: &SemanticNode, desired_state: SemanticWaitState) -> bool {
    match desired_state {
        SemanticWaitState::Enabled => node_is_enabled(node),
    }
}

fn node_is_enabled(node: &SemanticNode) -> bool {
    !node
        .attributes
        .as_ref()
        .is_some_and(|attrs| attrs.contains_key("disabled"))
}

fn state_contains_intent(root: &SemanticNode, intent: &str) -> bool {
    let normalized_intent = intent.trim().to_lowercase();
    node_contains_intent(root, &normalized_intent)
}

fn node_contains_intent(node: &SemanticNode, intent: &str) -> bool {
    if node
        .label
        .as_deref()
        .is_some_and(|label| label.trim().to_lowercase() == intent)
    {
        return true;
    }

    if let Some(attrs) = &node.attributes {
        for (key, value) in attrs {
            let key_normalized = key.to_lowercase();
            let value_normalized = value.to_lowercase();
            if (key_normalized == "data-intent" || key_normalized == "intent")
                && value_normalized == intent
            {
                return true;
            }
        }
    }

    for child in &node.children {
        if node_contains_intent(child, intent) {
            return true;
        }
    }

    false
}

fn error_chain_contains_any(err: &anyhow::Error, markers: &[&str]) -> bool {
    err.chain().any(|source| {
        let message = source.to_string().to_lowercase();
        markers.iter().any(|marker| message.contains(marker))
    })
}

fn is_transient_capture_error(err: &anyhow::Error) -> bool {
    let transient_markers = [
        "could not find node",
        "no node with given id",
        "execution context was destroyed",
        "cannot find context with specified id",
        "navigation",
    ];

    error_chain_contains_any(err, &transient_markers)
}

fn remaining_timeout(started: Instant, timeout: Duration) -> Duration {
    timeout.saturating_sub(started.elapsed())
}

fn value_to_u64(value: &serde_json::Value) -> Result<u64> {
    if let Some(v) = value.as_u64() {
        return Ok(v);
    }

    if let Some(v) = value.as_i64() {
        return u64::try_from(v).context("Expected non-negative integer value");
    }

    anyhow::bail!("Expected integer value, received: {value}");
}

fn duration_to_millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_state_contains_intent_exact_match() {
        let node = SemanticNode {
            role: "body".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "data-intent".to_string(),
                "checkout_complete".to_string(),
            )])),
            ..Default::default()
        };

        assert!(state_contains_intent(&node, "checkout_complete"));
        assert!(!state_contains_intent(&node, "signup_complete"));
    }

    #[test]
    fn test_node_is_enabled_with_disabled_attribute() {
        let disabled = SemanticNode {
            role: "button".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "disabled".to_string(),
                "".to_string(),
            )])),
            ..Default::default()
        };
        let enabled = SemanticNode {
            role: "button".to_string(),
            attributes: Some(std::collections::BTreeMap::from([(
                "id".to_string(),
                "btn_login".to_string(),
            )])),
            ..Default::default()
        };

        assert!(!node_is_enabled(&disabled));
        assert!(node_is_enabled(&enabled));
    }

    #[test]
    fn test_transient_capture_error_detection() {
        let transient = anyhow::anyhow!("Execution context was destroyed while loading");
        let non_transient = anyhow::anyhow!("Unsupported action: drag");
        let closed = anyhow::anyhow!("Target closed");

        assert!(is_transient_capture_error(&transient));
        assert!(!is_transient_capture_error(&non_transient));
        assert!(!is_transient_capture_error(&closed));
    }

    #[test]
    fn test_state_contains_intent_is_exact_match() {
        let node = SemanticNode {
            role: "text".to_string(),
            label: Some("checkout_complete_failed".to_string()),
            ..Default::default()
        };

        assert!(!state_contains_intent(&node, "checkout_complete"));
    }
}

#[cfg(test)]
mod browser_tests {
    use super::*;

    #[test]
    fn test_browser_initialization() {
        // This test requires a browser installed, so we might want to skip it if strictly unit testing logic
        // But for now, let's see if it compiles and runs in the environment
        if std::env::var("CI").is_ok() {
            return; // Skip in CI without browser setup
        }
        let browser = BrowserClient::new();
        assert!(browser.is_ok());
    }
}
