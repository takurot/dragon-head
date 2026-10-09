//! Embeds the reference Slack HITL bridge (`hitl-bridge` crate) inside the `dragon-head-mcp`
//! process, sharing this process's own `PageSession` (ISSUE-302).
//!
//! Previously, running `dragon-head-hitl-bridge` alongside `dragon-head-mcp` produced two
//! independent `BrowserClient -> PageSession` pairs: a policy approval raised by an action in
//! the MCP server lived in one session, while the standalone bridge polled a second, unrelated
//! session and could never see it. Spawning the bridge here instead, against
//! [`CoreRuntimeBackend::page_provider`], means both `ask_human` (in this process) and the Slack
//! bridge observe and resolve the exact same pending approval — `PageSession`'s approval methods
//! (`pending_policy_approval`, `approve_pending_policy_action`, `reject_pending_policy_action`)
//! take `&self` and are already safe to share across threads (`hitl-bridge`'s own standalone
//! binary already shares one `Arc<PageSession>` between its poll thread and HTTP handlers).
//!
//! See `docs/hitl-slack-bridge.md` for the supported deployment topologies.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};

use hitl_bridge::audit::BridgeAuditTrail;
use hitl_bridge::bridge::{run_poll_loop, Bridge};
use hitl_bridge::gateway::{ApprovalGateway, PageSessionGateway, SessionProvider};
use hitl_bridge::notifier::{ChatNotifier, SlackNotifier};
use hitl_bridge::server::{router, ServerState};

use crate::config::HitlBridgeConfig;

/// Starts the embedded HITL bridge on background threads: a poll loop that notifies Slack of
/// new pending approvals, and an Axum HTTP server that serves `/slack/interactions` for
/// Approve/Reject callbacks. Both operate on `page`, the same `PageSession` the caller's
/// `CoreRuntimeBackend` uses for `ask_human`.
///
/// The notifier, the audit log path (must be writable), the tokio runtime, and the
/// `config.bind_addr` listener (bound and registered
/// with the runtime) are all set up before any thread is spawned, and a failure in any of
/// them is returned. Once this returns `Ok`,
/// later failures inside the threads (a transient poll error, the server stopping) are logged
/// via `tracing` rather than propagated.
///
/// `page` is re-resolved on every poll and every Approve/Reject, so when the host's
/// browser-restart recovery (ISSUE-149) replaces its `PageSession` the bridge follows the
/// relaunched session (ISSUE-336). An approval still pending on the crashed session is lost
/// with it.
pub fn spawn_embedded_bridge(page: SessionProvider, config: &HitlBridgeConfig) -> Result<()> {
    let gateway: Arc<dyn ApprovalGateway> = Arc::new(PageSessionGateway::with_provider(page));
    let notifier: Arc<dyn ChatNotifier> = match &config.local_slack_api_base_url {
        Some(base_url) => Arc::new(
            SlackNotifier::with_local_api_base_url(
                config.slack_bot_token.clone(),
                config.slack_channel.clone(),
                base_url,
            )
            .context("failed to configure local HITL notifier")?,
        ),
        None => Arc::new(SlackNotifier::new(
            config.slack_bot_token.clone(),
            config.slack_channel.clone(),
        )),
    };
    let audit = BridgeAuditTrail::new(config.audit_log.clone());
    audit
        .verify_writable()
        .context("HITL bridge audit_log is not usable")?;
    let bind_addr = config.bind_addr.clone();
    let listener = std::net::TcpListener::bind(&bind_addr)
        .with_context(|| format!("failed to bind HITL bridge on {bind_addr}"))?;
    // Judge exposure by the address actually bound: `bind_addr` may be a hostname.
    if let Ok(local) = listener.local_addr() {
        if is_beyond_loopback(&local) {
            tracing::warn!(
                bind_addr = %local,
                "HITL bridge approval callback is bound beyond loopback; requests rely solely \
                 on Slack signature verification"
            );
        }
    }
    listener
        .set_nonblocking(true)
        .with_context(|| format!("failed to configure HITL bridge listener on {bind_addr}"))?;
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime for the HITL bridge")?;
    let listener = {
        let _enter = runtime.enter();
        tokio::net::TcpListener::from_std(listener)
            .with_context(|| format!("failed to register HITL bridge listener on {bind_addr}"))?
    };

    let bridge = Arc::new(Bridge::new(gateway, notifier, audit));

    let poll_bridge = Arc::clone(&bridge);
    let poll_interval = Duration::from_millis(config.poll_interval_ms);
    std::thread::spawn(move || {
        run_poll_loop(&poll_bridge, poll_interval, || false);
    });

    let state = ServerState {
        bridge,
        signing_secret: Arc::new(config.slack_signing_secret.clone()),
    };
    let app = router(state);
    std::thread::spawn(move || {
        runtime.block_on(async move {
            tracing::info!(
                addr = %bind_addr,
                "embedded hitl-bridge listening for Slack interactions"
            );
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "embedded hitl-bridge: Slack interactions server failed");
            }
        });
    });
    Ok(())
}

/// True when `addr` is not a loopback address (e.g. `0.0.0.0` or a LAN address).
fn is_beyond_loopback(addr: &std::net::SocketAddr) -> bool {
    !addr.ip().is_loopback()
}

#[cfg(test)]
mod tests {
    use super::is_beyond_loopback;
    use std::net::{SocketAddr, TcpListener};

    fn bound(bind_addr: &str) -> SocketAddr {
        TcpListener::bind(bind_addr)
            .expect("bind")
            .local_addr()
            .expect("local_addr")
    }

    #[test]
    fn loopback_binds_do_not_warn() {
        assert!(!is_beyond_loopback(&bound("127.0.0.1:0")));
        assert!(!is_beyond_loopback(&bound("localhost:0")));
    }

    #[test]
    fn wide_binds_warn() {
        assert!(is_beyond_loopback(&bound("0.0.0.0:0")));
    }
}
