use core_runtime::BrowserClient;
use std::{thread, time::Duration};

#[test]
fn test_browser_launch_and_navigate() -> anyhow::Result<()> {
    // Skip if CI environment (unless specific flags are set), as github actions might not have chrome installed by default in the 'test' job
    // But 'cdp-smoke' job installs it. Let's make it conditional or just try.
    // For local dev, we assume chrome is present.

    // Check if we are in a CI environment without chrome
    if test_bench_support::should_skip_browser_tests() {
        println!("Skipping CDP test: Chrome not available");
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let mut last_observation = String::new();
    let mut validated = false;

    for _ in 0..3 {
        page.navigate("https://example.com")?;
        let title = page.get_title()?;
        let content = page.get_content()?;
        last_observation = format!("https://example.com title={title:?}");

        if contains_example_domain(&title, &content) {
            validated = true;
            break;
        }

        page.navigate("http://example.com")?;
        let title = page.get_title()?;
        let content = page.get_content()?;
        last_observation = format!("http://example.com title={title:?}");

        if contains_example_domain(&title, &content) {
            validated = true;
            break;
        }

        thread::sleep(Duration::from_millis(200));
    }

    assert!(
        validated,
        "failed to validate example.com page after retries; {last_observation}"
    );

    Ok(())
}

fn contains_example_domain(title: &str, content: &str) -> bool {
    let title_normalized = title.to_lowercase();
    let content_normalized = content.to_lowercase();
    title_normalized.contains("example domain")
        || content_normalized.contains("example domain")
        || content_normalized.contains("<h1>example domain</h1>")
}
