# Incident Response Runbook

**Last updated:** 2026-06-28

Use this runbook when a production or evaluation workflow using
`dragon-head-mcp` fails in a way that affects safety, auditability, or
availability.

## Chrome crash or disconnect

Relevant behavior: PR-30 added typed Chrome crash/disconnect recovery. When an
MCP tool call encounters a disconnected browser process, `mcp-server` attempts
to relaunch Chrome through `CoreRuntimeBackend::handle_browser_disconnect`.

Expected effects:

- the current call returns a structured JSON-RPC error indicating that the
  browser restarted or that restart failed;
- `get_usage_report` increments `browser_restarts` after a successful restart;
- `AuditEvent::BrowserRestart` is written to the audit logger;
- in-page navigation state, cookies outside the Session Vault, DOM node IDs,
  state cache, and speculative snapshots are reset;
- policy rules are reapplied to the new page.

Response steps:

1. Check the MCP client error. If it reports `BrowserRestarted`, retry the
   workflow from a safe checkpoint rather than blindly replaying the last
   mutation.
2. Call `get_usage_report` and record `browser_restarts`.
3. Check stderr and persistent audit logs for `BrowserRestart` details.
4. Re-establish required page state: navigate to the target URL, reload session
   credentials from the vault if used, and call `get_state` with a suitable
   load profile.
5. If restarts repeat, stop the MCP client and inspect Chrome availability with
   `dragon-head-mcp --doctor`.

## Audit log write failure handling

Persistent audit logs are created only when `AUDIT_LOG_DIR` or
`audit.log_dir` is configured. If the rolling file sink cannot be created, the
server emits an `[AUDIT][ERROR]` line to stderr and falls back to in-memory
audit retention.

Response steps:

1. Treat persistent audit loss as a compliance-impacting incident for workflows
   that require durable action records.
2. Stop high-risk automation until the sink is restored.
3. Check directory existence, permissions, free disk space, and mount health for
   `AUDIT_LOG_DIR`.
4. Fix the path or permissions and restart the MCP client.
5. Run a low-risk `get_state` / `act` flow and confirm a new
   `audit_<unix_ms>_<sequence>.ndjson` file receives events.
6. If `AUDIT_DURABILITY=sync` causes unacceptable latency, switch to `flush`
   only after documenting the durability tradeoff for that deployment.

For `WebhookSink` SIEM delivery, failed HTTP delivery is best-effort and logs
`[AUDIT][ERROR]` to stderr after retries. Restore SIEM availability and compare
local rolling files with the SIEM ingestion window to identify gaps.

## HITL escalation timeout handling

HITL approvals can enter a pending state when policy requires human approval or
when an action cannot be safely disambiguated. The MCP tool surface exposes
`ask_human`, and the Slack/Teams reference bridge can poll and route approvals.

Response steps:

1. Inspect the failed or pending tool response for `requires_human_approval` or
   `ask_human_required`.
2. Confirm the approver channel or bridge is running. For Slack, check the
   `hitl-bridge` process, its `SLACK_*` environment variables, and its audit
   log path.
3. If the bridge is unavailable, pause the workflow. Do not auto-approve by
   replaying the mutation outside policy.
4. If the approval is stale, reject or abandon the pending request and restart
   from a fresh `get_state` so the human sees current page state.
5. After resolution, verify that one decision was recorded in the bridge audit
   log for the approval request.

## Browser restart rate limit

`mcp-server` limits browser restarts to 3 attempts within 60 seconds. The
counter includes failed relaunch attempts so crash loops cannot spin forever.

When the limit is hit, the backend returns an error similar to:

```text
browser restart rate limit exceeded (3 restarts within 60s); the Chrome process may be crash-looping
```

Response steps:

1. Stop the MCP client to break the crash loop.
2. Run `dragon-head-mcp --doctor` outside the client.
3. Check whether Chrome was updated, removed, quarantined, or blocked by host
   policy.
4. Check system resources: memory pressure, process limits, disk space, and
   sandbox restrictions.
5. Restart the MCP client only after `--doctor` passes.
6. If the issue returns on a specific page, reproduce with a narrow browser
   integration test or a minimal URL and file a bug with stderr, audit events,
   Chrome version, OS, and reproduction steps.

Do not raise the restart limit as a first response. The limit is a safety
guardrail that prevents repeated browser relaunches from hiding an underlying
host or page-specific crash.
