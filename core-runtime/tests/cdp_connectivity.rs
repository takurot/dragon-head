use core_runtime::BrowserClient;
use std::{thread, time::Duration};

/// A local fixture (no network dependency — see below) standing in for a
/// real page: exercises the exact CDP round-trip (navigate → get_title →
/// get_content) this test validates.
const FIXTURE_HTML: &str = "<html><head><title>Example Domain</title></head>\
<body><h1>Example Domain</h1></body></html>";

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

    // Use a `data:` URL fixture rather than a real external site: this test
    // validates the CDP navigate/get_title/get_content round-trip itself,
    // not whether the CI runner has working internet access. Hitting a real
    // external URL made this test a hard network dependency — flaky under
    // DNS hiccups, external outages, or a firewalled runner — for no benefit
    // to what's actually being asserted (issue #281).
    let url = format!("data:text/html,{}", urlencoding::encode(FIXTURE_HTML));

    let mut last_observation = String::new();
    let mut validated = false;

    for _ in 0..3 {
        page.navigate(&url)?;
        let title = page.get_title()?;
        let content = page.get_content()?;
        last_observation = format!("title={title:?}");

        if contains_example_domain(&title, &content) {
            validated = true;
            break;
        }

        thread::sleep(Duration::from_millis(200));
    }

    assert!(
        validated,
        "failed to validate fixture page after retries; {last_observation}"
    );

    Ok(())
}

fn contains_example_domain(title: &str, content: &str) -> bool {
    let title_normalized = title.to_lowercase();
    let content_normalized = content.to_lowercase();
    title_normalized.contains("example domain") || content_normalized.contains("example domain")
}
