use super::*;

impl PageSession {
    pub fn navigate(&self, url: &str) -> Result<()> {
        self.begin_action_navigation()?;
        let previous_url = self.current_url().ok();
        self.inner.navigate_to(url).context("Failed to navigate")?;
        if let Err(wait_error) = self.inner.wait_until_navigated() {
            self.wait_for_navigation_fallback(url, previous_url.as_deref())
                .with_context(|| {
                    format!("Failed to wait for navigation (primary wait error: {wait_error})")
                })?;
        }
        self.clear_stable_key_index();
        self.clear_semantic_capture_cache();
        self.navigation_epoch.fetch_add(1, Ordering::Relaxed);
        self.clear_pending_policy_approval();
        Ok(())
    }

    /// Navigate an untrusted public MCP destination through URL/network validation,
    /// destination-aware policy evaluation, and top-level redirect interception.
    ///
    /// Unlike [`navigate`](Self::navigate), this boundary accepts only HTTP(S).
    /// The internal method intentionally retains support for `data:` fixtures.
    pub fn navigate_public(&self, url: &str, allow_private_network: bool) -> Result<String> {
        self.navigate_with_persistent_boundary(url, allow_private_network)
    }

    pub fn public_navigation_attempt_count(&self) -> u64 {
        self.public_navigation_attempts.load(Ordering::Relaxed)
    }

    pub fn get_content(&self) -> Result<String> {
        self.inner
            .get_content()
            .context("Failed to get page content")
    }

    pub fn get_title(&self) -> Result<String> {
        self.inner.get_title().context("Failed to get page title")
    }

    pub fn current_url(&self) -> Result<String> {
        let value = self.evaluate_script_value("window.location.href", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve current page URL for policy evaluation")
    }

    pub(super) fn document_ready_state(&self) -> Result<String> {
        let value = self.evaluate_script_value("document.readyState", false)?;
        value
            .as_str()
            .map(ToOwned::to_owned)
            .context("Failed to resolve document.readyState while waiting for navigation")
    }

    pub(super) fn wait_for_navigation_fallback(
        &self,
        requested_url: &str,
        previous_url: Option<&str>,
    ) -> Result<()> {
        let started = Instant::now();
        let (last_url, last_ready_state, last_dom_non_empty) = loop {
            let current_url = self.current_url().ok();
            let ready_state = self.document_ready_state().ok();
            let dom_non_empty = self
                .get_content()
                .map(|content| !content.trim().is_empty())
                .unwrap_or(false);

            if navigation_fallback_condition_met(
                requested_url,
                previous_url,
                current_url.as_deref(),
                ready_state.as_deref(),
                dom_non_empty,
            ) {
                return Ok(());
            }

            let remaining = remaining_timeout(started, NAVIGATION_FALLBACK_TIMEOUT);
            if remaining.is_zero() {
                break (current_url, ready_state, dom_non_empty);
            }
            thread::sleep(min(NAVIGATION_FALLBACK_POLL_INTERVAL, remaining));
        };

        anyhow::bail!(
            "Navigation fallback timed out after {}ms (requested_url={}, previous_url={:?}, current_url={:?}, ready_state={:?}, dom_non_empty={})",
            duration_to_millis(NAVIGATION_FALLBACK_TIMEOUT),
            requested_url,
            previous_url,
            last_url,
            last_ready_state,
            last_dom_non_empty
        );
    }
}
