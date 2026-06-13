//! Integration tests for Chrome crash/disconnect recovery (ISSUE-149).
use anyhow::Context;
use core_runtime::BrowserClient;
use mcp_server::{CoreRuntimeBackend, McpBackend, McpServer};
use serde_json::json;
use std::time::Duration;

#[test]
fn call_tool_recovers_from_chrome_crash_and_relaunches() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate("https://example.com")?;

    let pid_before = client
        .process_id()
        .expect("launched BrowserClient should have a process id");

    let mut server = McpServer::new(CoreRuntimeBackend::new_with_client(client, page));

    std::process::Command::new("kill")
        .arg("-9")
        .arg(pid_before.to_string())
        .status()
        .context("failed to signal the Chrome process")?;

    // Bounded retry: 10 attempts / 200ms apart / 2s total, matching
    // core-runtime/tests/browser_recovery.rs, until the CDP transport
    // observes the disconnect and call_tool relaunches the session.
    let mut restarted_message = None;
    for _ in 0..10 {
        match server.call_tool("get_state", json!({})) {
            Ok(_) => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => {
                let message = err.to_string();
                if message.contains("automatically restarted") {
                    restarted_message = Some(message);
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let restarted_message =
        restarted_message.expect("expected call_tool to report an automatic restart");
    assert!(restarted_message.contains("restart #1"));

    // The session should now be usable again on a freshly relaunched Chrome.
    let state = server
        .call_tool("get_state", json!({}))
        .expect("get_state should succeed after relaunch");
    assert!(state.is_object());

    let usage = server
        .call_tool("get_usage_report", json!({}))
        .expect("get_usage_report should succeed");
    assert_eq!(usage["browser_restarts"], 1);

    Ok(())
}

#[test]
fn handle_browser_disconnect_without_managed_client_reports_unsupported() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let mut backend = CoreRuntimeBackend::new(page);
    let err = backend
        .handle_browser_disconnect()
        .expect_err("backend without a managed BrowserClient cannot restart");
    assert!(err.contains("no managed BrowserClient"), "error: {err}");
    assert_eq!(backend.browser_restart_count(), 0);

    Ok(())
}

#[test]
fn handle_browser_disconnect_rate_limits_after_max_restarts() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let page = client.new_page()?;

    let mut backend = CoreRuntimeBackend::new_with_client(client, page);

    for expected_count in 1..=3 {
        let restart_count = backend
            .handle_browser_disconnect()
            .expect("restart should succeed within the rate limit");
        assert_eq!(restart_count, expected_count);
    }

    let err = backend
        .handle_browser_disconnect()
        .expect_err("a 4th restart within the window should be rate limited");
    assert!(err.contains("rate limit"), "error: {err}");
    assert!(err.contains("crash-looping"), "error: {err}");
    assert_eq!(backend.browser_restart_count(), 3);

    Ok(())
}
