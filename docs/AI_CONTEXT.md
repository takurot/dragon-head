# AI_CONTEXT.md

**Last updated:** 2026-07-06

Read this first. It is a map for deciding which files to open next; it is not a
full manual.

## What This Project Is

Dragon Head is an AI-native headless browser runtime for LLM/VLM agents. It
wraps a Chrome/Chromium CDP session and exposes compact, structured
**Semantic State** through the `dragon-head-mcp` stdio MCP server. Agents use
the MCP tools to inspect pages, act on stable targets, verify outcomes, request
human approval, run declarative skills, extract structured data, and inspect
usage meters.

The current MCP tool contract has 8 tools, defined in `McpServer::tools()` in
`mcp-server/src/lib.rs`:

- `get_state`
- `act`
- `verify`
- `get_visual`
- `ask_human`
- `run_skill`
- `get_usage_report`
- `extract`

## Primary Users

- AI coding or browsing agents connected through MCP clients such as Claude
  Desktop, Claude Code, Codex, or any MCP-compatible client.
- Rust developers embedding `core-runtime` directly without the MCP layer.

## Tech Stack

- **Language:** Rust stable, pinned by `rust-toolchain.toml`.
- **Workspace:** 8 crates listed in the root `Cargo.toml`.
- **Browser control:** `headless_chrome` over CDP; Chrome/Chromium is required
  at runtime and is not vendored.
- **Protocol:** MCP JSON-RPC over stdio.
- **Plugins:** WebAssembly via `wasmtime` in `plugin-host`.
- **Testing:** `cargo nextest`, crate-local integration tests, and
  browser-dependent tests that skip unless Chrome is available.
- **CI:** GitHub Actions in `.github/workflows/ci.yml`, `e2e.yml`, and
  `release.yml`.

## Common Commands

```bash
just check
just test
just test-ci
just lint
just fmt
just test-all
cargo run -p mcp-server --bin dragon-head-mcp -- --doctor
```

There is no `just build` recipe in the current `Justfile`; use `cargo build`.

## Key Directories

| Path | What it is |
|---|---|
| `core-runtime/` | Chrome/CDP session, Semantic State, policy, audit, privacy, plugins, speculative state |
| `mcp-server/` | `dragon-head-mcp` binary, MCP tool dispatch, config, doctor/init commands |
| `skills-engine/` | Declarative browser workflow definitions and execution |
| `plugin-host/` | Wasm plugin manifest validation and sandboxed execution |
| `marketplace/` | Plugin/domain-pack metadata and revenue-share primitives |
| `hitl-bridge/` | Slack/Teams human approval bridge for `ask_human` flows |
| `bench/`, `nfr-baseline/` | Performance benchmarks and regression baselines |
| `docs/` | Spec, architecture, operational docs, and agent guidance |
| `examples/` | Chrome-free examples and MCP request/response fixtures |

## Files To Read First

1. `docs/AI_CONTEXT.md`
2. `docs/ARCHITECTURE.md`
3. `docs/FILE_GUIDE.md`
4. `GEMINI.md`
5. Local `AGENTS.md` / `CLAUDE.md`, if present. These are ignored by
   `.gitignore`, so treat them as operator-specific guidance, not repository
   source-of-truth documentation.
6. `docs/SPEC.md`, when behavior or public contract is relevant.
7. `docs/PLAN.md`, for historical PR/phase status only. Verify current code
   before assuming a feature is present or absent.

## Design Principles

- Preserve the `SemanticState` contract. Agents should not need raw DOM/HTML,
  and `stable_key` identity must remain stable across re-renders.
- Preserve audit-before-policy-before-mutation ordering in `act`.
- Keep per-session state inside `PageSession`; avoid hidden global caches.
- Capture metering, audit, and speculative state before propagating errors.
- Do not mutate process-wide environment variables in parallel tests; pass
  values through explicit parameters.

## High Blast-Radius Areas

- `core-runtime/src/sre/stable_key.rs`
- `core-runtime/src/browser.rs`
- `core-runtime/src/speculative/`
- `core-runtime/src/prompt_injection.rs`
- `.config/nextest.toml`
- `nfr-baseline/*.json`

Update documentation that names MCP tools, Semantic State schema, or developer
commands whenever the corresponding implementation changes.
