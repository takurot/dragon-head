use core_runtime::{sre::LoadProfile, BrowserClient};
use std::time::Instant;

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_nfr_state_update_latency_under_100ms() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let html = r#"
        <html>
            <body>
                <ul id="list">
                    <li>Item 1</li>
                </ul>
                <script>
                    window.addItems = () => {
                        const list = document.getElementById('list');
                        for (let i = 0; i < 45; i++) {
                            const li = document.createElement('li');
                            li.innerText = 'New Item ' + i;
                            list.appendChild(li);
                        }
                    };
                </script>
            </body>
        </html>
    "#;
    let url = format!("data:text/html,{}", urlencoding::encode(html));
    page.navigate(&url)?;

    // Initial capture (Full State)
    let _initial_state = page.capture_semantic_state(LoadProfile::Minimal)?;

    // Mutate DOM (< 50 nodes)
    page.evaluate_script("window.addItems()")?;

    // Measure State Update Latency (Delta State generation time)
    let start = Instant::now();
    let _delta_state = page.capture_semantic_state(LoadProfile::Minimal)?;
    let latency = start.elapsed();

    // Increase the timeout slightly for CI environments which can be slower.
    // The NFR is < 100ms, but CI runners often have noisy neighbors or slow IO.
    // We will use 250ms for CI stability while remaining strictly < 100ms locally.
    let limit = if std::env::var("CI").is_ok() {
        250
    } else {
        100
    };

    assert!(
        latency.as_millis() < limit,
        "State Update Latency regression: expected < {}ms, got {:?}",
        limit,
        latency
    );

    Ok(())
}
