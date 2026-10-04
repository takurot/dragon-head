use std::net::TcpListener;
use std::sync::Arc;

use core_runtime::{BrowserClient, PageSession};
use mcp_server::config::HitlBridgeConfig;
use mcp_server::hitl::spawn_embedded_bridge;

fn bridge_config(bind_addr: String, audit_dir: &tempfile::TempDir) -> HitlBridgeConfig {
    HitlBridgeConfig {
        bind_addr,
        slack_signing_secret: "test-signing-secret".to_string(),
        slack_bot_token: "xoxb-test".to_string(),
        slack_channel: "C0TEST".to_string(),
        audit_log: audit_dir.path().join("audit.ndjson"),
        poll_interval_ms: 60_000,
        local_slack_api_base_url: None,
    }
}

fn page() -> anyhow::Result<Arc<PageSession>> {
    let client = BrowserClient::new()?;
    Ok(Arc::new(client.new_page()?))
}

#[test]
fn embedded_bridge_startup_reports_bind_and_notifier_failures() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }
    let page = page()?;
    let audit_dir = tempfile::tempdir()?;

    let occupied = TcpListener::bind("127.0.0.1:0")?;
    let occupied_addr = occupied.local_addr()?.to_string();
    let err = spawn_embedded_bridge(
        Arc::clone(&page),
        &bridge_config(occupied_addr.clone(), &audit_dir),
    )
    .expect_err("binding a port that is already in use must fail");
    assert!(
        format!("{err:#}").contains(&occupied_addr),
        "error must name the bind address, got: {err:#}"
    );

    let err = spawn_embedded_bridge(
        Arc::clone(&page),
        &bridge_config("not-an-address".to_string(), &audit_dir),
    )
    .expect_err("an unparseable bind address must fail");
    assert!(
        format!("{err:#}").contains("not-an-address"),
        "error must name the bind address, got: {err:#}"
    );

    let mut bad_notifier = bridge_config("127.0.0.1:0".to_string(), &audit_dir);
    bad_notifier.local_slack_api_base_url = Some("https://slack.example.com/api".to_string());
    let err = spawn_embedded_bridge(Arc::clone(&page), &bad_notifier)
        .expect_err("a non-local Slack API override must fail instead of starting the bridge");
    assert!(
        format!("{err:#}").contains("local HITL notifier"),
        "error must name the notifier, got: {err:#}"
    );

    let free_addr = {
        let probe = TcpListener::bind("127.0.0.1:0")?;
        probe.local_addr()?.to_string()
    };
    spawn_embedded_bridge(page, &bridge_config(free_addr.clone(), &audit_dir))?;
    assert!(
        TcpListener::bind(&free_addr).is_err(),
        "the bridge must hold {free_addr} once spawn_embedded_bridge returns Ok"
    );
    Ok(())
}
