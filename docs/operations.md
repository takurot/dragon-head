# Operations Guide

**Last updated:** 2026-06-28

This guide covers day-to-day operation of the `dragon-head-mcp` stdio MCP
server after installation. For installation commands and MCP client snippets,
start with the root `README.md`.

## Starting and stopping the MCP server

`dragon-head-mcp` is a stdio server. In normal operation it is started and
stopped by the MCP client, not by a long-running service manager.

Start it from an MCP client configuration:

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp",
      "env": {
        "CHROME_PATH": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      }
    }
  }
}
```

Before enabling the client entry, validate the local runtime:

```bash
dragon-head-mcp --doctor
dragon-head-mcp --init codex
```

Stop the server by closing or reloading the MCP client session. If a client
leaves a process behind, terminate the `dragon-head-mcp` process and restart
the client. Running `dragon-head-mcp` directly in a terminal without stdin is a
smoke test only; it exits when stdin closes.

## Chrome path configuration and detection failures

Chrome/Chromium is launched through the `headless_chrome` crate. Detection uses
the default platform locations unless overridden.

Configuration precedence:

1. `CHROME_PATH` environment variable.
2. `chrome_path` in `$XDG_CONFIG_HOME/dragon-head/config.toml`, falling back to
   `$HOME/.config/dragon-head/config.toml`.
3. Built-in Chrome/Chromium detection.

Validate detection with:

```bash
dragon-head-mcp --doctor
```

Common failures:

| Symptom | Check |
| --- | --- |
| `--doctor` cannot find Chrome | Install Chrome/Chromium or set `CHROME_PATH` to the executable path. |
| GUI MCP client works in terminal but not in the app | Use an absolute `command` path or the `npx` form; GUI apps often do not inherit shell `PATH`. |
| Wrong browser build starts | Set `CHROME_PATH` explicitly in the MCP client `env` block or `config.toml`. |
| Config parse failure | Run `dragon-head-mcp --doctor`; malformed `config.toml`, invalid `prompt_injection.mode`, and invalid audit durability are fatal. |

## Log locations

There are two log surfaces: process diagnostics and audit events.

### MCP stderr

`dragon-head-mcp` prints startup, configuration, and warning messages to
stderr. The location depends on the parent MCP client:

- Terminal smoke run: visible in the terminal.
- GUI MCP clients: captured by the client process or its app logs.
- Service wrappers: captured by the wrapper's configured stderr sink.

Expected startup lines include:

```text
dragon-head-mcp: starting...
dragon-head-mcp: ready, listening on stdio
```

Security and audit misconfiguration warnings also go to stderr, for example
non-default prompt-injection mode warnings or audit sink creation failures.

### Audit log files

Persistent audit logging is off unless an audit log directory is configured.
Enable rolling NDJSON audit files with either environment variables or
`config.toml`:

```bash
export AUDIT_LOG_DIR=/var/log/dragon-head
export AUDIT_LOG_MAX_BYTES=10485760
export AUDIT_DURABILITY=flush
```

```toml
[audit]
log_dir = "/var/log/dragon-head"
max_bytes = 10485760
durability = "flush"
```

Files are created as:

```text
audit_<unix_ms>_<sequence>.ndjson
```

`AUDIT_LOG_MAX_BYTES` controls rotation. `AUDIT_DURABILITY=flush` flushes the
process buffer after each event; `sync` also calls `sync_data()` for stronger
crash durability at lower throughput.

If the audit directory cannot be created or opened, the server logs an
`[AUDIT][ERROR]` message to stderr and falls back to in-memory audit retention
so the MCP server can continue running.

The Slack/Teams reference bridge has a separate audit trail controlled by its
`--audit-log` argument; see [`hitl-slack-bridge.md`](hitl-slack-bridge.md).

## Session Vault key management procedure

`SessionVault` stores session cookies and token data through the
`core-runtime::session_vault` trait. The default `BrowserClient` uses
`LocalSessionVault` with a randomly generated in-process `SoftwareKms` key.
That default is suitable for one running MCP process, but it is not a durable
cross-process credential store.

Operational rules:

1. Treat vault data as secret material. Do not write decrypted session payloads
   to logs, fixtures, PR comments, or screenshots.
2. Use `save_to_vault(session_id)` only for sessions that are allowed to be
   reused by the current MCP process.
3. Use unique, purpose-specific session IDs. Avoid embedding user secrets in
   the session ID.
4. For integrations that need durable storage, provide an explicit
   `SessionVault` implementation and a KMS-backed `KmsAdapter`; do not rely on
   the default in-memory vault. Adapters that support vault-managed rotation
   must implement `AtomicKmsRotation` and return it from `atomic_rotation`;
   adapters without that capability fail closed instead of retaining old keys.
5. Rotate keys by calling `rotate_key(new_key, new_key_id)` on the vault
   implementation. New generic vault integrations should prefer
   `rotate_key_secret` so pending key bytes remain zeroizing even if the future
   is cancelled. Verify that existing sessions still load before retiring the
   previous key material.
6. After suspected key exposure, stop the MCP client, revoke the affected
   website sessions upstream, rotate vault keys, and restart the client.

Chrome crash recovery preserves the vault handle across relaunches, but the new
page starts with a fresh browser session. Reload required credentials from the
vault explicitly when the workflow depends on authenticated state after a
restart.

## Upgrading dragon-head-mcp safely

1. Read the release notes and check for MCP protocol, config, policy, audit, or
   prompt-injection mode changes.
2. Run the current binary's health checks and record the output:

   ```bash
   dragon-head-mcp --version
   dragon-head-mcp --doctor
   ```

3. Install the new binary through the same channel used for the old one:
   npm, GitHub release asset, install script, or source build.
4. Verify checksum files for downloaded release assets before replacing the
   binary.
5. Run the new binary checks:

   ```bash
   dragon-head-mcp --version
   dragon-head-mcp --doctor
   dragon-head-mcp --init codex
   ```

6. Restart the MCP client so it launches the new binary.
7. Confirm the client can call `get_state` on a known safe page and that stderr
   contains no new configuration or audit warnings.
8. Keep the previous binary available until the first real workflow completes.

If the new version changes `config.toml` or policy rule semantics, update the
file first and run `--doctor` before restarting the MCP client.
