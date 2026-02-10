use core_runtime::BrowserClient;

#[test]
fn test_browser_launch_and_navigate() -> anyhow::Result<()> {
    // Skip if CI environment (unless specific flags are set), as github actions might not have chrome installed by default in the 'test' job
    // But 'cdp-smoke' job installs it. Let's make it conditional or just try.
    // For local dev, we assume chrome is present.

    // Check if we are in a CI environment without chrome
    if std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err() {
        println!("Skipping CDP test in CI without CHROME_INSTALLED env");
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    page.navigate("https://example.com")?;

    let title = page.get_title()?;
    assert!(title.contains("Example Domain"));

    let content = page.get_content()?;
    assert!(content.contains("<h1>Example Domain</h1>"));

    Ok(())
}
