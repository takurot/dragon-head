# High-risk action demo

Last updated: 2026-10-01.

This offline E2E runs the shipped `dragon-head-mcp` binary over stdio against
a local expense form. Chrome, PolicyEngine, the embedded HITL bridge, signature
verification and audit persistence are real. A local Slack API double and a
scripted reviewer replace the hosted Slack service and a person. No payment or
external Slack message is sent.

## Run

Requires Rust, `just`, and installed Chrome/Chromium. From the repository root:

```sh
export CHROME_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
just demo-high-risk-action
```

On Linux, set `CHROME_PATH` to your installed Chrome/Chromium binary instead.
The test is `#[ignore]`-gated so an ordinary workspace test does not silently
claim this browser E2E ran. The demo command explicitly executes it and fails if
Chrome is unavailable. CI's `high-risk-action-demo` job runs it explicitly too.

## What it proves

1. A real MCP handshake, `navigate`, `get_state`, and `act` typing fill dummy
   email, password, card and amount fields. The amount is displayed as $900.00
   beside the submit action so outcome projection sees the same page context.
2. The otherwise-allowed submit crosses a $500 warning threshold. Its result is
   `requires_human_approval`, with projected amount 900 and high risk; the form
   has not submitted.
3. The embedded bridge polls the same browser session and sends a real Slack
   `chat.postMessage` request to the local API double. The test extracts the
   actual approval UUID from its buttons rather than manufacturing an approval.
4. An invalid signature is rejected without executing the action or recording
   a resolution. A valid, signed `/slack/interactions` callback grants approval
   and updates the same prompt with the scripted reviewer's decision.
5. Approval alone does not submit. Retrying the MCP action submits exactly once
   and produces the fixture receipt.
6. Persisted runtime NDJSON contains the ordered action attempts, policy
   escalation and HITL request, with typed values masked. The bridge's separate
   NDJSON resolution record matches the UUID, reviewer and projection. Neither
   trail contains the fixture's raw email, password or card value.

The runtime and bridge logs are two correlated trails, not a fabricated unified
event stream. Runtime audit currently does not record every MCP read call or a
separate approval-granted event; the bridge record supplies the reviewer and
approval decision. This demo checks the executed action/escalation sequence,
not universal audit completeness or regulatory compliance.

The test uses temporary configuration/audit directories, ephemeral loopback
services, child-only dummy credentials, bounded waits and process cleanup.
The fixture is [high-risk-payment.html](../mcp-server/tests/fixtures/high-risk-payment.html).

## Local transport safety

The embedded bridge's optional `[hitl_bridge].local_slack_api_base_url` exists
for this offline transport double. It accepts only HTTP literal loopback IPs
with an explicit nonzero port and the exact `/api` path; DNS aliases, remote
hosts, credentials, queries and fragments are rejected. For example:

```toml
[hitl_bridge]
local_slack_api_base_url = "http://127.0.0.1:18081/api"
```

This is an illustrative test endpoint, not a production Slack configuration.
Use dummy credentials only with it. The local client disables proxy inheritance
and redirects, and rejects non-2xx or malformed success responses before a
notification is accepted. Without this option, the bridge uses Slack's normal
`https://slack.com/api` endpoint. See [Slack bridge deployment](hitl-slack-bridge.md)
for real Slack setup and the supported shared-session topology.
