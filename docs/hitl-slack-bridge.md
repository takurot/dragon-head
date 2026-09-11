# Slack HITL Bridge (`hitl-bridge`)

`hitl-bridge` is a reference crate that demonstrates how an enterprise can route
Dragon Head's `ask_human` HITL (Human-In-The-Loop) approvals to a chat tool. It
satisfies spec section ACT-05 ("HITL Concurrency & Safety"): session-level exclusive
locks against double-approval, and an immutable audit trace of approver user ID,
timestamp, and Outcome Projection data.

The reference implementation targets **Slack**. `ChatNotifier` is a small trait
boundary — a Microsoft Teams notifier (Adaptive Cards) is a documented extension
point; implement the trait against the Teams Bot Framework API and wire it in place
of `SlackNotifier`.

## Deployment topology (ISSUE-302)

`hitl-bridge`'s poll loop and Slack webhook resolve approvals against whichever
`PageSession` its `ApprovalGateway` points at. That session must be the **same**
`PageSession` the agent's `ask_human` calls are pending against, or the bridge has
nothing to observe.

- **Embedded (recommended when using `dragon-head-mcp`)**: set `HITL_BRIDGE_ENABLED=true`
  (plus `SLACK_SIGNING_SECRET`/`SLACK_BOT_TOKEN`/`SLACK_CHANNEL`) on `dragon-head-mcp`
  itself. `main.rs` starts the same `Bridge`/poll-loop/webhook-server this crate
  exposes, on background threads inside the MCP server process, built from
  `CoreRuntimeBackend::page_handle()` — the exact `PageSession` `ask_human` uses. See
  the [README's config reference](../README.md#precedence) for the full env var list.
  This is the only supported way to run the bridge alongside `dragon-head-mcp`.
- **Standalone (`dragon-head-hitl-bridge` binary)**: launches its **own**
  `BrowserClient`/`PageSession`, independent of any other process. Running it next to
  a separately-launched `dragon-head-mcp` does **not** work — the two processes each
  own an unrelated session, so a policy approval raised by an action in one is
  invisible to the other. The standalone binary remains useful for driving `hitl-bridge`
  against a session that isn't otherwise part of an MCP server (a bespoke embedding,
  a test harness), or for local development against `tests/bridge_flow.rs`'s
  `page_session_gateway_resolves_a_real_pending_approval`-style setup.

Known limitation: the embedded mode's gateway is built once, from whichever
`PageSession` is live at `dragon-head-mcp` startup. If Chrome crashes and
`dragon-head-mcp`'s own restart recovery (ISSUE-149) relaunches it with a fresh
`PageSession`, the embedded bridge keeps observing the old, now-defunct session — any
pending approval on it becomes moot, and a *new* approval raised after the restart
requires restarting `dragon-head-mcp` itself to be picked up. Tracked as a follow-up;
not addressed by ISSUE-302, which scopes to eliminating the two-independent-sessions
bug for the normal (no browser crash) case.

## How it works

```
PageSession (ask_human pending) ──poll──▶ Bridge ──notify──▶ Slack channel (Block Kit)
                                             │                        │
                                             │                        ▼
                                             │              reviewer clicks Approve/Reject
                                             │                        │
                                             ▼                        ▼
                                  ResolutionRegistry ◀── POST /slack/interactions
                                             │
                    (owner / exact retry) ──▶ gateway.approve()/reject() ──▶ audit trail ──▶ chat update
```

1. **Polling** (`Bridge::poll_once`, `bridge.rs`): on a fixed interval (default 1s),
   the bridge asks the `ApprovalGateway` for the current `pending_policy_approval()`.
   On a new request it mints a stable `Uuid` (keyed by `(rule_id, target_signature,
   action)` since `PolicyApprovalRequest` carries no native ID) and posts a Block Kit
   prompt via `ChatNotifier::notify`.
2. **Interaction webhook** (`server.rs`): `POST /slack/interactions` is the external
   trust boundary. Every request must carry a valid `X-Slack-Signature` HMAC-SHA256
   over `v0:{timestamp}:{body}`, verified in constant time and checked for staleness
   (≤5 minutes, matching Slack's documented recommendation). Verification fails
   closed — malformed, unsigned, stale, or mis-signed requests are rejected (`401`)
   before any gateway/lock/audit state is touched.
3. **Resolution** (`Bridge::resolve`): enforces resumable phases — **claim the
   request → mutate the gateway (approve/reject) → write the audit record → update
   the chat message**. The first reviewer and decision own the request. If a phase
   fails, only that exact reviewer/decision pair may retry, starting at the first
   incomplete phase; competing decisions never repeat an earlier side effect.

## Message format

The Slack prompt (Block Kit, `notifier::build_approval_blocks`) contains:

- **Reason / Action intent** — the policy `rule_id` that required approval and the
  action it gates (e.g. `` `click` ``).
- **Outcome Projection** — the Guardian Angel projection attached to the
  `HumanApprovalRequired` error: projected amount (`$900.50`) and risk level
  (`Low`/`Medium`/`High`/`Critical`), or a `(not available)` placeholder when no
  projection was captured.
- **Set-of-Mark capture** (optional) — a screenshot of the page at request time,
  rendered as an inline `data:image/png;base64,...` image block. **Production note**:
  inline base64 images bloat message payloads; production deployments should upload
  via Slack's `files.upload` API and reference the returned URL instead. The
  reference implementation inlines the image to avoid the extra round trip and keep
  the example self-contained (`som_image_png` is currently always `None` — wiring a
  live `VisualCapture` into the notification is a follow-up for a bridge that owns
  its own `BrowserClient` capture cadence).
- **Approve / Reject buttons** — `action_id: "approve"` / `"reject"`, with `value`
  set to the bridge-minted request `Uuid` so the interaction handler can correlate
  the click back to the pending request.

Once resolved, the original message is replaced (`chat.update`) with a static
`*Resolved:* approved by *alice*` / `*Resolved:* rejected by *bob*` line.

## Session-lock / "first decision wins" semantics

`Bridge` keeps a per-request `ResolutionProgress` behind a mutex. Creating the entry
atomically claims the request for one `(decided_by, decision)` pair. A competing pair
is told who already resolved it and cannot touch the gateway, audit trail, or chat.
The owning pair may retry an incomplete `gateway_applied`, `audited`, or
`chat_updated` phase; completed phases are skipped. The registry is capped at 1,024
entries. Completed history is evicted oldest-first, while a registry full of
unrepaired failures rejects new claims instead of growing without bound. This is what
`tests/bridge_flow.rs::concurrent_resolutions_of_the_same_request_apply_exactly_once`
and the fail-once phase tests in `bridge.rs` verify.

## Audit trail format

`BridgeAuditTrail` (`audit.rs`) is an append-only NDJSON log — one immutable JSON
record per line, opened in append mode and `fsync`'d before a write reports success.
An identical retry for an existing request ID is idempotent; conflicting decision
data for the same ID is rejected:

```json
{"id":"5b1b...","decision":"approved","decided_by":"alice","decided_at_ms":1717740000000,"outcome_projection":{"projected_amount":900.5,"risk_level":"high"}}
```

Each record carries the approver's user ID (`decided_by` — Slack `username`, falling
back to `user.id` when no username is set), the resolution timestamp
(`decided_at_ms`), and the Outcome Projection captured at resolution time
(`outcome_projection`) — satisfying ACT-05's "approver user ID, timestamp, and
Outcome Projection data in an immutable log" requirement. The bridge keeps its own
log rather than widening the shared `core_runtime::AuditEvent` enum for a
reference-only consumer.

## Running it

### 1. Create a Slack App

1. Go to <https://api.slack.com/apps> → **Create New App** → "From scratch".
2. **OAuth & Permissions**: add the `chat:write` bot scope, install the app to your
   workspace, and copy the **Bot User OAuth Token** (`xoxb-...`).
3. **Basic Information**: copy the **Signing Secret**.
4. **Interactivity & Shortcuts**: turn interactivity on and set the **Request URL**
   to `https://<your-host>/slack/interactions` (the bridge's bind address must be
   reachable from Slack — use a tunnel such as `ngrok` for local testing).
5. Invite the bot to the target channel and note the channel ID.

### 2a. Run it embedded in `dragon-head-mcp` (recommended)

```bash
HITL_BRIDGE_ENABLED=true \
SLACK_SIGNING_SECRET=... \
SLACK_BOT_TOKEN=xoxb-... \
SLACK_CHANNEL=C0123456789 \
cargo run -p mcp-server --bin dragon-head-mcp
```

`HITL_BRIDGE_BIND_ADDR`, `HITL_BRIDGE_AUDIT_LOG`, and `HITL_BRIDGE_POLL_INTERVAL_MS`
(or the equivalent `[hitl_bridge]` `config.toml` keys — see the
[README's config reference](../README.md#precedence)) override the `0.0.0.0:8787` /
`hitl-bridge-audit.ndjson` / `1000`ms defaults. Slack credentials are env-only by
design and are never read from `config.toml`.

The default `0.0.0.0:8787` binds on all interfaces (matching the standalone binary's
own default below). Every request to it is HMAC-signature-verified before any
approval mutation, but for anything beyond local development, set
`HITL_BRIDGE_BIND_ADDR=127.0.0.1:8787` and put a reverse proxy or tunnel (e.g.
`ngrok`) in front of it, rather than exposing the port directly.

### 2b. Run the standalone binary (its own, unrelated session)

```bash
SLACK_SIGNING_SECRET=... \
SLACK_BOT_TOKEN=xoxb-... \
SLACK_CHANNEL=C0123456789 \
cargo run -p hitl-bridge -- \
  --bind-addr 0.0.0.0:8787 \
  --audit-log hitl-bridge-audit.ndjson \
  --poll-interval-ms 1000
```

Run `cargo run -p hitl-bridge -- --help` for the full CLI reference (all Slack
credentials are also configurable via the `SLACK_*` environment variables shown
above, via `clap`'s `env` attribute). See "Deployment topology" above: this launches
its own `BrowserClient`/`PageSession` and will not see approvals raised by a
separately-running `dragon-head-mcp`.

### 3. Exercise the interaction endpoint locally

To observe the lock/audit behavior without a live Slack workspace, send a correctly
signed `block_actions` payload directly:

```bash
TS=$(date +%s)
BODY="payload=$(python3 -c 'import json,urllib.parse;print(urllib.parse.quote(json.dumps({"user":{"id":"U999","username":"alice"},"actions":[{"action_id":"approve","value":"<request-uuid>"}]})))')"
SIG="v0=$(printf 'v0:%s:%s' "$TS" "$BODY" | openssl dgst -sha256 -hmac "$SLACK_SIGNING_SECRET" | sed 's/^.* //')"

curl -s -o /dev/null -w '%{http_code}\n' \
  -H "X-Slack-Request-Timestamp: $TS" \
  -H "X-Slack-Signature: $SIG" \
  -H 'Content-Type: application/x-www-form-urlencoded' \
  --data "$BODY" \
  http://localhost:8787/slack/interactions
```

A `200` indicates all resolution phases completed. A `409` indicates either that a
different reviewer/decision owns the request or that the owning decision hit a
retryable gateway, audit, or chat-update failure; a `401`/`400` indicates signature
verification or payload parsing failed (check the bridge's `tracing` logs for the
rejection reason).

## Evaluation-bench exemption

Following the precedent set by `bench/` (PR-28, also a standalone reference crate),
`hitl-bridge` is not registered in the comprehensive evaluation bench
(`docs/testing.md` §2.1's "Registration Rule" targets the five core crates —
`core-runtime`, `mcp-server`, `skills-engine`, `plugin-host`, `marketplace`). Its
correctness is covered by its own unit and integration suites
(`cargo test -p hitl-bridge`).
