//! Integration tests for Chrome crash/disconnect recovery (ISSUE-149).
//!
//! These tests kill the underlying Chrome process to simulate a crash, then
//! verify that `is_browser_disconnected` detects the resulting CDP error and
//! that `BrowserClient::relaunch` recovers with a fresh process and page.
use anyhow::Context;
use core_runtime::audit::AuditLogger;
use core_runtime::{is_browser_disconnected, BrowserClient};
use std::time::Duration;

// Uses the Unix `kill` command to simulate a Chrome crash; not supported on Windows.
#[cfg(unix)]
#[test]
fn relaunch_recovers_session_after_chrome_process_is_killed() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let mut client = BrowserClient::new()?;
    let page = client.new_page()?;
    page.navigate("https://example.com")?;

    let pid_before = client
        .process_id()
        .expect("launched BrowserClient should have a process id");

    std::process::Command::new("kill")
        .arg("-9")
        .arg(pid_before.to_string())
        .status()
        .context("failed to signal the Chrome process")?;

    // The CDP transport's background thread needs a moment to notice the
    // closed websocket. Bounded retry: 10 attempts / 200ms apart / 2s total.
    let mut disconnect_err = None;
    for _ in 0..10 {
        match page.get_document_node() {
            Ok(_) => std::thread::sleep(Duration::from_millis(200)),
            Err(err) => {
                if is_browser_disconnected(&err) {
                    disconnect_err = Some(err);
                    break;
                }
                std::thread::sleep(Duration::from_millis(200));
            }
        }
    }
    let disconnect_err =
        disconnect_err.expect("expected a disconnect error after killing the Chrome process");
    assert!(is_browser_disconnected(&disconnect_err));

    let new_page = client.relaunch(AuditLogger::new(), "chrome process killed in test", 1)?;

    let pid_after = client
        .process_id()
        .expect("relaunched BrowserClient should have a process id");
    assert_ne!(
        pid_before, pid_after,
        "relaunch should start a new Chrome process"
    );

    new_page.navigate("https://example.com")?;
    let title = new_page.get_title()?;
    assert!(!title.is_empty(), "relaunched page should be usable");

    Ok(())
}

/// `BrowserClient::confirm_alive` (ISSUE-261) should confirm a healthy,
/// freshly-launched browser as alive well within its bound.
#[test]
fn confirm_alive_reports_true_for_healthy_browser() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    assert!(
        client.confirm_alive(Duration::from_secs(3)),
        "a freshly launched browser should confirm alive"
    );
    Ok(())
}

/// After the Chrome process is killed, `confirm_alive` should eventually
/// report `false` (ISSUE-261) -- proving the health-check correctly
/// distinguishes a genuine crash from a merely slow-but-alive transport.
#[cfg(unix)]
#[test]
fn confirm_alive_reports_false_after_chrome_process_is_killed() -> anyhow::Result<()> {
    if test_bench_support::should_skip_browser_tests() {
        return Ok(());
    }

    let client = BrowserClient::new()?;
    let _page = client.new_page()?;

    let pid = client
        .process_id()
        .expect("launched BrowserClient should have a process id");
    std::process::Command::new("kill")
        .arg("-9")
        .arg(pid.to_string())
        .status()
        .context("failed to signal the Chrome process")?;

    // The transport's background thread needs a moment to notice the
    // closed websocket. Bounded retry: 10 attempts / 200ms apart / 2s total,
    // each individual health check itself bounded to 2s.
    let mut confirmed_dead = false;
    for _ in 0..10 {
        if !client.confirm_alive(Duration::from_secs(2)) {
            confirmed_dead = true;
            break;
        }
        std::thread::sleep(Duration::from_millis(200));
    }
    assert!(
        confirmed_dead,
        "confirm_alive should report false after the Chrome process is killed"
    );
    Ok(())
}
