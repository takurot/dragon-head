use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::{
    cmp::min,
    sync::Arc,
    thread,
    time::{Duration, Instant},
};

use crate::{
    error::WaitError,
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
        Ok(PageSession { inner: tab })
    }
}

pub struct PageSession {
    inner: Arc<headless_chrome::Tab>,
}

impl PageSession {
    pub fn navigate(&self, url: &str) -> Result<()> {
        self.inner.navigate_to(url).context("Failed to navigate")?;
        self.inner
            .wait_until_navigated()
            .context("Failed to wait for navigation")?;
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
                    let err_str = e.to_string();
                    let is_node_error = err_str.contains("Could not find node")
                        || err_str.contains("No node with given id");

                    if !is_node_error {
                        return Err(e);
                    }

                    if stable_key.is_none() {
                        return Err(e);
                    }
                    // Fallback proceed...
                }
            }
        }

        // Fallback: use stable_key
        if let Some(key) = stable_key {
            // Re-fetch DOM and normalize to find new ID
            // Using Interactive profile to ensure we catch most elements (including those needing JS/styles).
            // Minimal might be too aggressive in filtering.
            let root = self.get_document_node()?;
            let sem_root = crate::sre::normalize_dom(crate::sre::LoadProfile::Interactive, &root)?;

            // Find node by key

            if let Some(new_id) = find_node_id_by_key(&sem_root, key) {
                // Log success of fallback
                eprintln!(
                    "[WARN] Action recovered via stable key: {} -> new_id: {}",
                    key, new_id
                );
                return self.perform_action_by_id(new_id, action, value);
            } else {
                // Both failures -> VerifyRequired
                return Err(crate::error::ActionError::VerifyRequired.into());
            }
        }

        // If we reach here, we had no stable key or it failed lookup, and target_id failed or wasn't provided.
        // Actually, if stable_key was None, we would have returned early in the target_id block if target_id was Some.
        // If target_id was None AND stable_key was None, we should also error.

        Err(crate::error::ActionError::VerifyRequired.into())
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
        let mut subscriber =
            SreEventSubscriber::new(self, options.load_profile, options.poll_interval);
        let started = Instant::now();

        loop {
            match subscriber.poll_next_state_event() {
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

            if started.elapsed() >= timeout {
                return Err(WaitError::Timeout {
                    operation: format!(
                        "semantic target {:?} to become {:?}",
                        target, desired_state
                    ),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            sleep_until_next_poll(started, timeout, subscriber.poll_interval());
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
        let mut subscriber =
            SreEventSubscriber::new(self, options.load_profile, options.poll_interval);
        let started = Instant::now();

        loop {
            match subscriber.poll_next_state_event() {
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

            if started.elapsed() >= timeout {
                return Err(WaitError::Timeout {
                    operation: format!("intent '{}'", intent),
                    timeout_ms: duration_to_millis(timeout),
                }
                .into());
            }

            sleep_until_next_poll(started, timeout, subscriber.poll_interval());
        }
    }

    fn capture_state(&self, profile: LoadProfile) -> Result<SemanticState> {
        let root = self.get_document_node()?;
        let sem_root = normalize_dom(profile, &root)?;
        Ok(SemanticState::new(sem_root, profile))
    }

    fn perform_action_by_id(
        &self,
        backend_node_id: i64,
        action: &str,
        value: Option<&str>,
    ) -> Result<()> {
        // Resolve backend_node_id to RemoteObject
        use headless_chrome::protocol::cdp::Runtime::CallFunctionOn;
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
            })?
            .object;

        let object_id = remote_object
            .object_id
            .context("Failed to resolve node to object")?;

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
    poll_interval: Duration,
    last_state_hash: Option<String>,
}

impl<'a> SreEventSubscriber<'a> {
    fn new(session: &'a PageSession, profile: LoadProfile, poll_interval: Duration) -> Self {
        Self {
            session,
            profile,
            poll_interval: if poll_interval.is_zero() {
                DEFAULT_WAIT_POLL_INTERVAL
            } else {
                poll_interval
            },
            last_state_hash: None,
        }
    }

    fn poll_next_state_event(&mut self) -> Result<Option<SemanticState>> {
        let state = self.session.capture_state(self.profile)?;
        let current_hash = state.state_hash().to_owned();

        if self.last_state_hash.as_deref() == Some(current_hash.as_str()) {
            return Ok(None);
        }

        self.last_state_hash = Some(current_hash);
        Ok(Some(state))
    }

    fn poll_interval(&self) -> Duration {
        self.poll_interval
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

fn find_node_id_by_key(node: &SemanticNode, target_key: &str) -> Option<i64> {
    find_node_by_key(node, target_key).map(|target| target.backend_node_id)
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
        .is_some_and(|label| label.to_lowercase().contains(intent))
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

fn is_transient_capture_error(err: &anyhow::Error) -> bool {
    let msg = err.to_string().to_lowercase();
    let transient_markers = [
        "could not find node",
        "no node with given id",
        "execution context was destroyed",
        "cannot find context with specified id",
        "inspected target navigated or closed",
        "target closed",
        "session closed",
        "navigation",
    ];

    transient_markers.iter().any(|marker| msg.contains(marker))
}

fn sleep_until_next_poll(started: Instant, timeout: Duration, poll_interval: Duration) {
    let elapsed = started.elapsed();
    if elapsed >= timeout {
        return;
    }

    let remaining = timeout.saturating_sub(elapsed);
    thread::sleep(min(poll_interval, remaining));
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

        assert!(is_transient_capture_error(&transient));
        assert!(!is_transient_capture_error(&non_transient));
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
