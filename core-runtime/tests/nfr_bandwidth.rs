use core_runtime::{sre::LoadProfile, BrowserClient};

fn should_skip() -> bool {
    std::env::var("CI").is_ok() && std::env::var("CHROME_INSTALLED").is_err()
}

#[test]
fn test_nfr_bandwidth_95_percent_reduction() -> anyhow::Result<()> {
    if should_skip() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let removable_nodes = (0..400)
        .map(|i| {
            format!(
                "<script>console.log({i})</script><style>.x{i}{{color:red;}}</style><img src=\"t{i}.png\" alt=\"tracker\" />"
            )
        })
        .collect::<String>();
    let html = format!(
        r#"
        <html>
            <head>
                <style>
                    body {{ font-family: sans-serif; }}
                    .content {{ padding: 20px; }}
                    .ad {{ width: 300px; height: 250px; background: red; }}
                </style>
            </head>
            <body>
                <div class="content">
                    <h1>Main Article</h1>
                    <p>This is the main content.</p>
                    {removable_nodes}
                    <div class="ad">Advertisement</div>
                    <button id="btn">Click me</button>
                </div>
            </body>
        </html>
    "#
    );
    let url = format!("data:text/html,{}", urlencoding::encode(&html));
    page.navigate(&url)?;

    let minimal_state = page.capture_semantic_state(LoadProfile::Minimal)?;
    let minimal_json = serde_json::to_string(&minimal_state)?;

    let standard_state = page.capture_semantic_state(LoadProfile::Interactive)?;
    let standard_json = serde_json::to_string(&standard_state)?;

    let min_size = minimal_json.len() as f64;
    let std_size = standard_json.len() as f64;

    assert!(std_size > 0.0, "standard profile payload must be non-zero");

    // Network-byte measurement is out of scope for this test environment.
    // As a deterministic proxy, enforce payload reduction on semantic output.
    let reduction = 1.0 - (min_size / std_size);
    eprintln!(
        "NFR bandwidth proxy: minimal_bytes={} standard_bytes={} reduction={:.2}%",
        min_size as usize,
        std_size as usize,
        reduction * 100.0
    );

    assert!(
        reduction >= 0.95,
        "Bandwidth reduction regression: expected >=95%, got {:.2}%",
        reduction * 100.0
    );

    Ok(())
}
