//! Embeds the reference Slack HITL bridge (`hitl-bridge` crate) inside the `dragon-head-mcp`
//! process, sharing this process's own `PageSession` (ISSUE-302).
//!
//! Previously, running `dragon-head-hitl-bridge` alongside `dragon-head-mcp` produced two
//! independent `BrowserClient -> PageSession` pairs: a policy approval raised by an action in
//! the MCP server lived in one session, while the standalone bridge polled a second, unrelated
//! session and could never see it. Spawning the bridge here instead, against
//! [`CoreRuntimeBackend::page_handle`], means both `ask_human` (in this process) and the Slack
//! bridge observe and resolve the exact same pending approval — `PageSession`'s approval methods
//! (`pending_policy_approval`, `approve_pending_policy_action`, `reject_pending_policy_action`)
//! take `&self` and are already safe to share across threads (`hitl-bridge`'s own standalone
//! binary already shares one `Arc<PageSession>` between its poll thread and HTTP handlers).
//!
//! See `docs/hitl-slack-bridge.md` for the supported deployment topologies.

use std::sync::Arc;
use std::time::Duration;

use core_runtime::PageSession;
use hitl_bridge::audit::BridgeAuditTrail;
use hitl_bridge::bridge::{run_poll_loop, Bridge};
use hitl_bridge::gateway::{ApprovalGateway, PageSessionGateway};
use hitl_bridge::notifier::{ChatNotifier, SlackNotifier};
use hitl_bridge::server::{router, ServerState};

use crate::config::HitlBridgeConfig;

/// Starts the embedded HITL bridge on background threads: a poll loop that notifies Slack of
/// new pending approvals, and an Axum HTTP server that serves `/slack/interactions` for
/// Approve/Reject callbacks. Both operate on `page`, the same `PageSession` the caller's
/// `CoreRuntimeBackend` uses for `ask_human`.
///
/// Returns as soon as the background threads are spawned; failures inside them (a bind error,
/// a panicked poll loop) are logged via `tracing` rather than propagated, matching the
/// standalone `dragon-head-hitl-bridge` binary's own poll-loop error handling (a transient
/// failure must not take the bridge, or the MCP server it's embedded in, down).
///
/// Known limitation (ISSUE-302 follow-up): `page` is a snapshot of whichever `PageSession` is
/// live when this is called. If the host's own browser-restart recovery (ISSUE-149) later
/// replaces its `PageSession`, this embedded bridge keeps observing the old, now-defunct
/// session rather than following the restart — the old session's pending approval (if any)
/// simply becomes unreachable, and any *new* approval raised after the restart requires
/// restarting `dragon-head-mcp` itself to pick up.
pub fn spawn_embedded_bridge(page: Arc<PageSession>, config: &HitlBridgeConfig) {
    let gateway: Arc<dyn ApprovalGateway> = Arc::new(PageSessionGateway::new(page));
    let notifier: Arc<dyn ChatNotifier> = Arc::new(SlackNotifier::new(
        config.slack_bot_token.clone(),
        config.slack_channel.clone(),
    ));
    let audit = BridgeAuditTrail::new(config.audit_log.clone());
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
    let bind_addr = config.bind_addr.clone();
    std::thread::spawn(move || {
        let runtime = match tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            Ok(runtime) => runtime,
            Err(err) => {
                tracing::error!(error = %err, "embedded hitl-bridge: failed to build tokio runtime");
                return;
            }
        };
        runtime.block_on(async move {
            let listener = match tokio::net::TcpListener::bind(&bind_addr).await {
                Ok(listener) => listener,
                Err(err) => {
                    tracing::error!(
                        error = %err,
                        addr = %bind_addr,
                        "embedded hitl-bridge: failed to bind Slack interactions server"
                    );
                    return;
                }
            };
            tracing::info!(
                addr = %bind_addr,
                "embedded hitl-bridge listening for Slack interactions"
            );
            if let Err(err) = axum::serve(listener, app).await {
                tracing::error!(error = %err, "embedded hitl-bridge: Slack interactions server failed");
            }
        });
    });
}
