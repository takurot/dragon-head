//! Reference Slack HITL bridge CLI.
//!
//! Wires a live `core_runtime::PageSession` to Slack: polls for pending
//! human-approval requests, posts Block Kit prompts with Outcome Projection
//! data, and serves `/slack/interactions` to relay Approve/Reject decisions
//! back into the session under a session-level exclusive lock with an
//! immutable audit trail (spec ACT-05). See `docs/hitl-slack-bridge.md`.

use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use clap::Parser;
use core_runtime::BrowserClient;

use hitl_bridge::audit::BridgeAuditTrail;
use hitl_bridge::bridge::{run_poll_loop, Bridge};
use hitl_bridge::gateway::{ApprovalGateway, PageSessionGateway};
use hitl_bridge::notifier::{ChatNotifier, SlackNotifier};
use hitl_bridge::server::{router, ServerState};

/// Reference implementation of the Slack/Teams HITL approval bridge (ACT-05 / ISSUE-15).
#[derive(Parser, Debug)]
#[command(name = "dragon-head-hitl-bridge", version, about)]
struct Cli {
    /// Address to bind the Slack interactivity webhook server to.
    #[arg(long, default_value = "0.0.0.0:8787")]
    bind_addr: String,

    /// Slack app signing secret, used to verify `X-Slack-Signature` on inbound interactions.
    #[arg(long, env = "SLACK_SIGNING_SECRET")]
    slack_signing_secret: String,

    /// Slack bot token (`xoxb-...`) used to post and update approval prompts.
    #[arg(long, env = "SLACK_BOT_TOKEN")]
    slack_bot_token: String,

    /// Slack channel ID to post approval prompts into.
    #[arg(long, env = "SLACK_CHANNEL")]
    slack_channel: String,

    /// Path to the append-only NDJSON audit trail file.
    #[arg(long, default_value = "hitl-bridge-audit.ndjson")]
    audit_log: std::path::PathBuf,

    /// How often to poll the session for new pending approval requests.
    #[arg(long, default_value_t = 1000)]
    poll_interval_ms: u64,

    /// Optional explicit Chrome binary path, forwarded to `BrowserClient`.
    #[arg(long)]
    chrome_path: Option<String>,
}

fn main() -> Result<()> {
    tracing_subscriber::fmt::init();
    let cli = Cli::parse();

    let client = BrowserClient::new_with_chrome_path(cli.chrome_path.clone())
        .context("failed to launch BrowserClient")?;
    let session = Arc::new(
        client
            .new_page()
            .context("failed to open a new page session")?,
    );

    let gateway: Arc<dyn ApprovalGateway> = Arc::new(PageSessionGateway::new(session));
    let notifier: Arc<dyn ChatNotifier> = Arc::new(SlackNotifier::new(
        cli.slack_bot_token.clone(),
        cli.slack_channel.clone(),
    ));
    let audit = BridgeAuditTrail::new(cli.audit_log.clone());

    let bridge = Arc::new(Bridge::new(gateway, notifier, audit));

    let poll_bridge = Arc::clone(&bridge);
    let poll_interval = Duration::from_millis(cli.poll_interval_ms);
    let poll_handle = std::thread::spawn(move || {
        run_poll_loop(&poll_bridge, poll_interval, || false);
    });

    let state = ServerState {
        bridge,
        signing_secret: Arc::new(cli.slack_signing_secret.clone()),
    };
    let app = router(state);

    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    runtime.block_on(async {
        let listener = tokio::net::TcpListener::bind(&cli.bind_addr)
            .await
            .with_context(|| format!("failed to bind {}", cli.bind_addr))?;
        tracing::info!(addr = %cli.bind_addr, "hitl-bridge listening for Slack interactions");
        axum::serve(listener, app)
            .await
            .context("Slack interactions server failed")
    })?;

    poll_handle.join().expect("poll thread panicked");
    Ok(())
}
