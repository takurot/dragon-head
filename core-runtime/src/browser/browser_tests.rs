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

#[tokio::test]
async fn test_session_vault_save_load() -> Result<()> {
    if !crate::chrome_available() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    page.navigate("https://example.com")?;

    // Set a cookie manually to test save/load
    page.inner
        .call_method(headless_chrome::protocol::cdp::Network::SetCookie {
            name: "test_cookie".to_string(),
            value: "test_value".to_string(),
            url: Some("https://example.com".to_string()),
            domain: None,
            path: None,
            secure: None,
            http_only: None,
            same_site: None,
            expires: None,
            priority: None,
            same_party: None,
            source_scheme: None,
            source_port: None,
            partition_key: None,
        })?;

    // Save to vault
    page.save_to_vault("my-session").await?;

    // Create a new page and load from vault
    let page2 = client.new_page()?;
    page2.navigate("https://example.com")?; // Navigate first so it has the right context

    // Clear existing browser cookies so the restore path is actually exercised.
    page2
        .inner
        .call_method(headless_chrome::protocol::cdp::Network::ClearBrowserCookies(None))?;
    let cookies_before = page2.inner.get_cookies()?;
    let found_before = cookies_before
        .iter()
        .any(|c| c.name == "test_cookie" && c.value == "test_value");
    assert!(!found_before, "Cookie unexpectedly present before restore");

    page2.load_from_vault("my-session").await?;

    // Verify cookie exists in page2
    let cookies = page2.inner.get_cookies()?;
    let found = cookies
        .iter()
        .any(|c| c.name == "test_cookie" && c.value == "test_value");
    assert!(found, "Cookie 'test_cookie' not found in restored session");

    Ok(())
}
