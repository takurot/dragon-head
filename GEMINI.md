# GEMINI.md - Dragon Head: Neural-Browser Runtime

## Project Overview

Dragon Head is an AI-native headless browser runtime for LLM/VLM agents. It
wraps Chrome/Chromium through CDP and converts pages into compact
**Semantic State** for agent consumption.

## Architecture Summary

- `core-runtime` owns Chrome/CDP sessions, Semantic Rendering Engine capture,
  policy enforcement, audit logging, privacy redaction, prompt-injection
  handling, speculative state, and plugin hooks.
- `mcp-server` exposes the `dragon-head-mcp` stdio MCP server, including tool
  dispatch, config loading, `--doctor`, `--init`, and usage metering.
- `skills-engine` runs declarative browser workflows.
- `plugin-host` validates and executes Wasm plugins.
- `marketplace`, `hitl-bridge`, `bench`, and `test-bench-support` provide
  commercial metadata, HITL integration, benchmarking, and test support.

## Common Commands

The project uses `just` for common development tasks.

| Command | Description |
| :--- | :--- |
| `just check` | Runs `cargo check` across the workspace. |
| `cargo build` | Builds the workspace. There is no `just build` recipe in the current `Justfile`. |
| `just fmt` | Formats all code using `cargo fmt`. |
| `just lint` | Runs `clippy` with `-D warnings`. |
| `just test` | Runs the workspace nextest suite. |
| `just test-ci` | Runs nextest with the CI profile. |
| `just test-all` | Runs tests, linting, and formatting checks. |

Use `cargo run -p mcp-server --bin dragon-head-mcp -- --doctor` to validate
Chrome detection and config parsing.

## Tests

Tests are crate-local, for example:

- `core-runtime/tests/`
- `mcp-server/tests/`
- `skills-engine/tests/`
- `plugin-host/tests/`
- `marketplace/tests/`
- `hitl-bridge/tests/`

Browser-dependent tests use `test-bench-support` helpers and skip when Chrome is
not available unless explicitly enabled.

## Documentation Source Of Truth

- Use `docs/AI_CONTEXT.md`, `docs/ARCHITECTURE.md`, and `docs/FILE_GUIDE.md`
  to orient before changing code.
- Treat `docs/PLAN.md` as historical phase/PR status. Verify current behavior
  in code, tests, and user-facing docs before making claims.
- Local `AGENTS.md` / `CLAUDE.md` files are ignored by `.gitignore`; if present,
  treat them as operator-specific supplemental instructions.

## Key Directories

- `core-runtime/`
- `mcp-server/`
- `skills-engine/`
- `plugin-host/`
- `marketplace/`
- `hitl-bridge/`
- `bench/`
- `test-bench-support/`
- `docs/`
- `examples/`
