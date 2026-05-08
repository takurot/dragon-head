# Dragon Head: Neural-Browser Runtime

**Last updated:** 2026-05-08

Dragon Head is an AI-native headless browser runtime for LLM and VLM agents.
It exposes a browser session as a compact, structured **Semantic State** and
provides an MCP server that agents can use to inspect pages, act on elements,
verify outcomes, request human approval, and run declarative skills.

The current user-facing entry point is the stdio MCP server binary:

```text
dragon-head-mcp
```

At the moment, Dragon Head is run from source. Prebuilt binaries and package
manager installation are tracked in [Issue #95](https://github.com/takurot/dragon-head/issues/95).

## Current Status

Implemented today:

- `dragon-head-mcp` stdio MCP server in the `mcp-server` crate.
- Core browser runtime backed by Chrome/Chromium through CDP.
- Semantic state generation, stable element identity, semantic delta delivery,
  visual capture, policy enforcement, audit logging, session vault support,
  PII redaction, self-healing action recovery, plugin hooks, and Skills Engine
  integration.
- Developer examples that demonstrate the core concepts without launching
  Chrome.

Roadmap items:

- GitHub Releases native binaries, Homebrew, Docker, and `cargo install`
  distribution.
- `dragon-head-mcp doctor` and MCP client init helpers.
- Deep Lens extraction DSL, Guardian Angel outcome projection, and speculative
  state generation as production-ready product surfaces.

## Install and Run

### Prerequisites

- Rust stable toolchain.
- Chrome or Chromium for the MCP server and browser-backed tests.

Dragon Head checks `CHROME_PATH` first. If it is not set, it tries common
Chrome/Chromium locations for macOS, Linux, and Windows.

Example macOS setting:

```bash
export CHROME_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

### Run the MCP server from source

```bash
git clone https://github.com/takurot/dragon-head.git
cd dragon-head
cargo run -p mcp-server --bin dragon-head-mcp
```

The server starts on stdio and prints lifecycle logs to stderr:

```text
dragon-head-mcp: starting...
dragon-head-mcp: ready, listening on stdio
```

For repeated local use, build a release binary:

```bash
cargo build -p mcp-server --bin dragon-head-mcp --release
./target/release/dragon-head-mcp
```

## MCP Client Setup

Dragon Head runs as a stdio MCP server. Your MCP client starts the command,
passes JSON-RPC messages on stdin, and reads responses from stdout.

### 1. Choose the command

Use this command while running from a source checkout:

```bash
cargo run --manifest-path /path/to/dragon-head/Cargo.toml -p mcp-server --bin dragon-head-mcp
```

Use this command after building a local release binary:

```bash
/path/to/dragon-head/target/release/dragon-head-mcp
```

After packaged releases ship, use the installed command directly:

```bash
dragon-head-mcp
```

### 2. Add the MCP server config

Most MCP clients expose an `mcpServers` JSON object. Use this
source-checkout configuration while binary releases are not available.
Replace `/path/to/dragon-head` with your absolute local checkout path.

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "cargo",
      "args": [
        "run",
        "--manifest-path",
        "/path/to/dragon-head/Cargo.toml",
        "-p",
        "mcp-server",
        "--bin",
        "dragon-head-mcp"
      ],
      "env": {
        "CHROME_PATH": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      }
    }
  }
}
```

For a locally built or packaged binary, the configuration becomes:

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

If Chrome is installed in a standard location, you can omit `CHROME_PATH`.
Set it explicitly when the MCP server cannot find Chrome or when you want to
use a specific Chromium build.

### 3. Restart the MCP client

After updating the client config, restart the MCP client so it launches
`dragon-head-mcp`. A successful startup writes these lifecycle logs to stderr:

```text
dragon-head-mcp: starting...
dragon-head-mcp: ready, listening on stdio
```

The client should then show the Dragon Head tools listed below.

### 4. Troubleshooting

- Use absolute paths in MCP config. Relative paths depend on the client process
  working directory and are easy to break.
- Put environment variables in the JSON `env` object. Do not rely on shell
  startup files being loaded by GUI clients.
- If startup fails before the tools appear, confirm Chrome is installed or set
  `CHROME_PATH`.
- If the source-checkout command is slow on every client launch, build the
  release binary and configure the client to run `target/release/dragon-head-mcp`.
- Running `dragon-head-mcp` directly in a terminal is only a startup smoke test;
  the server is designed to be managed by an MCP client over stdio.

## Available MCP Tools

`dragon-head-mcp` currently exposes these tools:

| Tool | Purpose |
| --- | --- |
| `get_state` | Retrieve the semantic page state. |
| `act` | Execute an interaction action. |
| `verify` | Verify precondition text before acting. |
| `get_visual` | Capture visual context with optional marks. |
| `ask_human` | Resolve a pending human-in-the-loop request. |
| `run_skill` | Execute a declarative skill workflow. |
| `get_usage_report` | Retrieve usage meters and plan tier summary. |

## Developer Examples

These examples do not require Chrome and are useful for understanding the data
model and policy behavior:

```bash
# Core concepts: SemanticState, Fast State, PolicyEngine, MCP-style payload
cargo run --example quickstart

# Policy rule cookbook
cargo run --example policy_cookbook
```

See [examples/README.md](examples/README.md) for sample policies, sample skills,
and MCP request/response fixtures.

## Testing and Verification

Run the full workspace test suite:

```bash
cargo test --workspace
```

Run formatting and lint checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

Browser-backed tests are skipped when Chrome is unavailable in supported test
paths, but the MCP server itself needs Chrome/Chromium to start a real browser
session.

## Architecture

Dragon Head is organized as a Rust workspace:

- `core-runtime`: Chrome/CDP browser session, semantic state capture, action
  execution, policy, audit, privacy, visual capture, and session vault.
- `mcp-server`: stdio MCP server and tool contract.
- `skills-engine`: declarative workflow execution.
- `plugin-host`: Wasm plugin validation and runtime execution.
- `marketplace`: plugin/domain-pack marketplace primitives.

## Distribution Plan

The intended low-friction distribution path is:

1. GitHub Releases native binaries for macOS, Linux, and Windows.
2. Homebrew tap for macOS installation.
3. Copy-paste MCP client configuration templates.
4. Optional install script for users who prefer a one-command setup.
5. Docker image for CI and Linux evaluation environments.
6. `cargo install` / crates.io path for Rust developers after workspace crate
   publishing is ready.

This work is tracked in [Issue #95](https://github.com/takurot/dragon-head/issues/95).

## Project Roadmap

Near-term:

- Package `dragon-head-mcp` as the primary install artifact.
- Add install verification and config helpers.
- Tighten README and examples around MCP client onboarding.

Product roadmap:

- Deep Lens zero-code extraction DSL.
- Guardian Angel outcome projection for proactive policy decisions.
- Speculative state generation for near-zero TTFT targets.
- Slack/Teams HITL reference integration.
- Shared Wasm engine and module caching.

---

This project is under active development.
