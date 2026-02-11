use anyhow::{Context, Result};
use headless_chrome::{Browser, LaunchOptions};
use std::sync::Arc;

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
}

#[cfg(test)]
mod tests {
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
