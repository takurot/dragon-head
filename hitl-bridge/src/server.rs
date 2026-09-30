//! Inbound HTTP surface: Slack interactivity webhook.
//!
//! `POST /slack/interactions` is an external trust boundary — every request
//! must carry a valid `X-Slack-Signature` HMAC-SHA256 over
//! `v0:{timestamp}:{body}` (Slack's signing-secret scheme). Verification
//! fails closed: malformed, unsigned, stale, or mis-signed requests are
//! rejected before any gateway/lock/audit state is touched.

use anyhow::{Context, Result};
use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::post,
    Router,
};
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use crate::bridge::Bridge;
use crate::lock::Decision;

/// Maximum age of a signed request before it is rejected as a replay.
///
/// Matches Slack's documented recommendation of five minutes.
const MAX_REQUEST_AGE_SECS: u64 = 60 * 5;

type HmacSha256 = Hmac<Sha256>;

#[derive(Clone)]
pub struct ServerState {
    pub bridge: Arc<Bridge>,
    pub signing_secret: Arc<String>,
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/slack/interactions", post(handle_interaction))
        .with_state(state)
}

/// Verifies `v0={hmac_sha256(signing_secret, "v0:{timestamp}:{body}")}` against
/// the `X-Slack-Signature` header, and rejects requests whose `X-Slack-Request-Timestamp`
/// is missing, malformed, or older than [`MAX_REQUEST_AGE_SECS`].
///
/// Returns `Ok(())` only when every check passes; any failure is reported as
/// a descriptive error so the caller can respond `401 Unauthorized` without
/// touching gateway, lock, or audit state (fail closed).
fn verify_slack_signature(headers: &HeaderMap, body: &[u8], signing_secret: &str) -> Result<()> {
    let timestamp_header = headers
        .get("x-slack-request-timestamp")
        .and_then(|value| value.to_str().ok())
        .context("missing X-Slack-Request-Timestamp header")?;
    let timestamp: u64 = timestamp_header
        .parse()
        .context("X-Slack-Request-Timestamp header is not a valid integer")?;

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before the Unix epoch")?
        .as_secs();
    let age = now
        .checked_sub(timestamp)
        .unwrap_or_else(|| timestamp - now);
    if age > MAX_REQUEST_AGE_SECS {
        anyhow::bail!(
            "request timestamp {timestamp} is too far from server time {now} (age={age}s)"
        );
    }

    let signature_header = headers
        .get("x-slack-signature")
        .and_then(|value| value.to_str().ok())
        .context("missing X-Slack-Signature header")?;
    let expected_hex = signature_header
        .strip_prefix("v0=")
        .context("X-Slack-Signature header is missing the 'v0=' version prefix")?;
    let expected_bytes =
        hex::decode(expected_hex).context("X-Slack-Signature header is not valid hex")?;

    let mut mac = HmacSha256::new_from_slice(signing_secret.as_bytes())
        .context("HMAC can take a key of any length")?;
    mac.update(b"v0:");
    mac.update(timestamp_header.as_bytes());
    mac.update(b":");
    mac.update(body);

    // `verify_slice` performs a constant-time comparison internally.
    mac.verify_slice(&expected_bytes)
        .map_err(|_| anyhow::anyhow!("signature mismatch"))?;

    Ok(())
}

/// Slack `block_actions` interaction payload — only the fields the bridge
/// needs are modeled; everything else is ignored.
#[derive(Debug, serde::Deserialize)]
struct BlockActionsPayload {
    user: InteractingUser,
    actions: Vec<BlockAction>,
}

#[derive(Debug, serde::Deserialize)]
struct InteractingUser {
    id: String,
    #[serde(default)]
    username: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct BlockAction {
    action_id: String,
    value: String,
}

fn parse_interaction(body: &[u8]) -> Result<(Uuid, Decision, String)> {
    let form: Vec<(String, String)> = serde_urlencoded::from_bytes(body)
        .context("failed to parse interaction body as form data")?;
    let payload_json = form
        .into_iter()
        .find(|(key, _)| key == "payload")
        .map(|(_, value)| value)
        .context("interaction body is missing the 'payload' field")?;

    let payload: BlockActionsPayload =
        serde_json::from_str(&payload_json).context("failed to parse interaction payload JSON")?;
    let action = payload
        .actions
        .into_iter()
        .next()
        .context("interaction payload contains no actions")?;

    let decision = match action.action_id.as_str() {
        "approve" => Decision::Approved,
        "reject" => Decision::Rejected,
        other => anyhow::bail!("unrecognized action_id '{other}'"),
    };
    let id: Uuid = action
        .value
        .parse()
        .context("action value is not a valid request ID")?;
    let decided_by = payload.user.username.unwrap_or(payload.user.id);

    Ok((id, decision, decided_by))
}

async fn handle_interaction(
    State(state): State<ServerState>,
    headers: HeaderMap,
    body: Bytes,
) -> StatusCode {
    if let Err(err) = verify_slack_signature(&headers, &body, &state.signing_secret) {
        tracing::warn!(error = %err, "rejected Slack interaction: signature verification failed");
        return StatusCode::UNAUTHORIZED;
    }

    let (id, decision, decided_by) = match parse_interaction(&body) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::warn!(error = %err, "rejected Slack interaction: malformed payload");
            return StatusCode::BAD_REQUEST;
        }
    };

    // Gateway, audit, and notifier phases can block (the Slack notifier uses
    // reqwest's blocking client). Keep them off the async HTTP executor.
    match tokio::task::spawn_blocking(move || state.bridge.resolve(id, decision, &decided_by)).await
    {
        Ok(Ok(())) => StatusCode::OK,
        Ok(Err(err)) => {
            tracing::warn!(error = %err, %id, "failed to resolve approval request");
            StatusCode::CONFLICT
        }
        Err(_) => {
            tracing::error!(%id, "approval resolution worker failed");
            StatusCode::INTERNAL_SERVER_ERROR
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn signed_headers(timestamp: u64, body: &[u8], secret: &str) -> HeaderMap {
        let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(b"v0:");
        mac.update(timestamp.to_string().as_bytes());
        mac.update(b":");
        mac.update(body);
        let signature = format!("v0={}", hex::encode(mac.finalize().into_bytes()));

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-slack-request-timestamp",
            timestamp.to_string().parse().unwrap(),
        );
        headers.insert("x-slack-signature", signature.parse().unwrap());
        headers
    }

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    #[test]
    fn valid_signature_with_fresh_timestamp_passes() {
        let body = b"payload=test-body";
        let headers = signed_headers(now_secs(), body, "shh-secret");

        assert!(verify_slack_signature(&headers, body, "shh-secret").is_ok());
    }

    #[test]
    fn signature_computed_with_wrong_secret_is_rejected() {
        let body = b"payload=test-body";
        let headers = signed_headers(now_secs(), body, "wrong-secret");

        assert!(verify_slack_signature(&headers, body, "shh-secret").is_err());
    }

    #[test]
    fn tampered_body_is_rejected() {
        let original_body = b"payload=test-body";
        let headers = signed_headers(now_secs(), original_body, "shh-secret");

        let tampered_body = b"payload=tampered-body";
        assert!(verify_slack_signature(&headers, tampered_body, "shh-secret").is_err());
    }

    #[test]
    fn stale_timestamp_is_rejected() {
        let body = b"payload=test-body";
        let stale_timestamp = now_secs() - MAX_REQUEST_AGE_SECS - 60;
        let headers = signed_headers(stale_timestamp, body, "shh-secret");

        assert!(verify_slack_signature(&headers, body, "shh-secret").is_err());
    }

    #[test]
    fn missing_signature_header_is_rejected() {
        let body = b"payload=test-body";
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-slack-request-timestamp",
            now_secs().to_string().parse().unwrap(),
        );

        assert!(verify_slack_signature(&headers, body, "shh-secret").is_err());
    }

    #[test]
    fn missing_version_prefix_is_rejected() {
        let body = b"payload=test-body";
        let mut mac = HmacSha256::new_from_slice(b"shh-secret").unwrap();
        mac.update(b"v0:");
        mac.update(now_secs().to_string().as_bytes());
        mac.update(b":");
        mac.update(body);
        let raw_hex = hex::encode(mac.finalize().into_bytes());

        let mut headers = HeaderMap::new();
        headers.insert(
            "x-slack-request-timestamp",
            now_secs().to_string().parse().unwrap(),
        );
        headers.insert("x-slack-signature", raw_hex.parse().unwrap());

        assert!(verify_slack_signature(&headers, body, "shh-secret").is_err());
    }

    #[test]
    fn parse_interaction_extracts_id_decision_and_actor() {
        let id = Uuid::new_v4();
        let payload = serde_json::json!({
            "user": { "id": "U999", "username": "alice" },
            "actions": [{ "action_id": "approve", "value": id.to_string() }],
        })
        .to_string();
        let body = serde_urlencoded::to_string([("payload", payload)]).unwrap();

        let (parsed_id, decision, decided_by) = parse_interaction(body.as_bytes()).unwrap();

        assert_eq!(parsed_id, id);
        assert_eq!(decision, Decision::Approved);
        assert_eq!(decided_by, "alice");
    }

    #[test]
    fn parse_interaction_falls_back_to_user_id_when_username_absent() {
        let id = Uuid::new_v4();
        let payload = serde_json::json!({
            "user": { "id": "U999" },
            "actions": [{ "action_id": "reject", "value": id.to_string() }],
        })
        .to_string();
        let body = serde_urlencoded::to_string([("payload", payload)]).unwrap();

        let (_, decision, decided_by) = parse_interaction(body.as_bytes()).unwrap();

        assert_eq!(decision, Decision::Rejected);
        assert_eq!(decided_by, "U999");
    }

    #[test]
    fn parse_interaction_rejects_unknown_action_id() {
        let payload = serde_json::json!({
            "user": { "id": "U999" },
            "actions": [{ "action_id": "snooze", "value": Uuid::new_v4().to_string() }],
        })
        .to_string();
        let body = serde_urlencoded::to_string([("payload", payload)]).unwrap();

        assert!(parse_interaction(body.as_bytes()).is_err());
    }

    #[test]
    fn signed_callback_with_real_http_notifier_resolves_outside_async_context() {
        use crate::audit::BridgeAuditTrail;
        use crate::gateway::{mock::MockGateway, PendingApproval};
        use crate::notifier::SlackNotifier;
        use std::io::{Read, Write};

        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        listener.set_nonblocking(true).unwrap();
        let api = std::thread::spawn(move || {
            for method in ["chat.postMessage", "chat.update"] {
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                let (mut stream, _) = loop {
                    match listener.accept() {
                        Ok(connection) => break connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            assert!(
                                std::time::Instant::now() < deadline,
                                "local API request timed out"
                            );
                            std::thread::sleep(std::time::Duration::from_millis(5));
                        }
                        Err(error) => panic!("local API accept failed: {error}"),
                    }
                };
                stream
                    .set_read_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(std::time::Duration::from_secs(5)))
                    .unwrap();
                let mut request = Vec::new();
                let mut buffer = [0; 4096];
                loop {
                    let count = stream.read(&mut buffer).unwrap();
                    assert!(count > 0);
                    request.extend_from_slice(&buffer[..count]);
                    if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                        let headers = String::from_utf8_lossy(&request[..end]);
                        let length = headers
                            .lines()
                            .find_map(|line| {
                                line.to_ascii_lowercase()
                                    .strip_prefix("content-length: ")
                                    .and_then(|v| v.parse::<usize>().ok())
                            })
                            .unwrap();
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                assert!(
                    String::from_utf8_lossy(&request).starts_with(&format!("POST /api/{method} "))
                );
                let body = r#"{"ok":true,"ts":"123.45"}"#;
                write!(stream, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}", body.len()).unwrap();
            }
        });
        let id = Uuid::new_v4();
        let gateway = Arc::new(MockGateway::new(Some(PendingApproval {
            id,
            rule_id: "demo".into(),
            action: "click".into(),
            target_signature: "submit".into(),
            scope: core_runtime::ApprovalScope::ActionOnly,
            outcome: None,
        })));
        let notifier = SlackNotifier::with_local_api_base_url(
            "dummy",
            "C-demo",
            &format!("http://{address}/api"),
        )
        .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let bridge = Arc::new(Bridge::new(
            gateway.clone(),
            Arc::new(notifier),
            BridgeAuditTrail::new(dir.path().join("audit.ndjson")),
        ));
        bridge.poll_once().unwrap();
        let payload = serde_json::json!({"user":{"id":"demo"},"actions":[{"action_id":"approve","value":id.to_string()}]});
        let body = serde_urlencoded::to_string([("payload", payload.to_string())]).unwrap();
        let headers = signed_headers(now_secs(), body.as_bytes(), "demo-secret");
        let state = ServerState {
            bridge: Arc::clone(&bridge),
            signing_secret: Arc::new("demo-secret".into()),
        };
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let status = runtime.block_on(handle_interaction(State(state), headers, Bytes::from(body)));
        assert_eq!(status, StatusCode::OK);
        assert_eq!(gateway.resolutions().len(), 1);
        api.join().unwrap();
    }

    #[test]
    fn signed_callback_maps_resolution_failure_and_worker_panic() {
        use crate::audit::BridgeAuditTrail;
        use crate::gateway::{ApprovalGateway, PendingApproval};
        use crate::notifier::mock::MockNotifier;

        struct FailingGateway(bool);
        impl ApprovalGateway for FailingGateway {
            fn pending_request(&self) -> Option<PendingApproval> {
                None
            }
            fn approve(&self, _: Uuid) -> Result<()> {
                assert!(!self.0, "intentional worker panic");
                anyhow::bail!("intentional gateway failure")
            }
            fn reject(&self, _: Uuid) -> Result<()> {
                anyhow::bail!("unused reject")
            }
        }

        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        for (panic, expected) in [
            (false, StatusCode::CONFLICT),
            (true, StatusCode::INTERNAL_SERVER_ERROR),
        ] {
            let dir = tempfile::tempdir().unwrap();
            let bridge = Arc::new(Bridge::new(
                Arc::new(FailingGateway(panic)),
                Arc::new(MockNotifier::new()),
                BridgeAuditTrail::new(dir.path().join("audit.ndjson")),
            ));
            let payload = serde_json::json!({"user":{"id":"demo"},"actions":[{"action_id":"approve","value":Uuid::new_v4().to_string()}]});
            let body = serde_urlencoded::to_string([("payload", payload.to_string())]).unwrap();
            let headers = signed_headers(now_secs(), body.as_bytes(), "demo-secret");
            let state = ServerState {
                bridge,
                signing_secret: Arc::new("demo-secret".into()),
            };
            assert_eq!(
                runtime.block_on(handle_interaction(State(state), headers, Bytes::from(body))),
                expected
            );
        }
    }
}
