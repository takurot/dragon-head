# AI_CONTEXT.md

> Read this file first. It is a map, not a manual — use it to decide which other
> file to open next, instead of exploring the whole repo.

## What this project is

Dragon Head is an AI-native **headless browser runtime** for LLM/VLM agents. It
wraps a Chrome/CDP session and exposes it to an agent as a compact, structured
**Semantic State** (an accessibility-tree-like JSON, not raw HTML/DOM) via a
stdio **MCP server** binary, `dragon-head-mcp`. The agent inspects pages, acts
on elements, verifies outcomes, requests human approval, and runs declarative
skills — all through 7 MCP tools.

## Primary users

- AI coding/browsing agents (Claude, GPT, etc.) connected via an MCP client
  (Claude Desktop, Claude Code, Codex, or any MCP-compatible client).
- Developers embedding `core-runtime` directly as a Rust library (no MCP).

## Primary use cases

- Web automation/testing driven by an LLM (navigate, click, fill forms, verify
  text) without feeding raw HTML/screenshots into the model.
- Human-in-the-loop (HITL) gated actions (e.g. financial transactions) via
  policy rules + Slack/Teams approval (`hitl-bridge`).
- Declarative, reusable browser workflows ("skills") instead of ad-hoc agent
  exploration on every run.

## Tech stack

- **Language**: Rust (stable, see `rust-toolchain.toml`), workspace of 8 crates.
- **Browser control**: `headless_chrome` (CDP), Chrome/Chromium required at
  runtime (not vendored).
- **Protocol**: MCP (JSON-RPC over stdio).
- **Plugins**: WebAssembly via `wasmtime` (`plugin-host`).
- **Testing**: `cargo nextest`, integration tests under `tests/` per crate.
- **CI**: GitHub Actions (`.github/workflows/ci.yml`, `e2e.yml`, `release.yml`).

## Key commands

```bash
just check              # cargo check --workspace
just test               # cargo nextest run --workspace
just test-ci             # nextest --profile ci (what CI runs)
just lint                # cargo clippy --workspace -- -D warnings
just fmt                 # cargo fmt --all
just test-all            # test + lint + fmt
cargo run -p mcp-server --bin dragon-head-mcp -- --doctor
```
See `DEVELOPMENT_GUIDE.md` for the full list.

## Key directories

| Path | What it is |
|---|---|
| `core-runtime/` | Chrome/CDP session, Semantic State, policy, audit, privacy, plugins, speculative state |
| `mcp-server/` | The `dragon-head-mcp` stdio binary and tool dispatch — **start here for tool behavior** |
| `skills-engine/` | Declarative skill (workflow) definitions and execution |
| `plugin-host/` | Wasm plugin manifest validation + sandboxed execution |
| `marketplace/` | Plugin/domain-pack metadata and revenue-share primitives |
| `hitl-bridge/` | Slack/Teams human-approval bridge for `ask_human` |
| `bench/`, `nfr-baseline/` | Performance benchmarking and regression baselines |
| `docs/` | Spec, plan, and this onboarding doc set |
| `examples/` | Runnable, Chrome-free examples (`cargo run --example quickstart`) |

## Files to read before changing anything

1. `AI_CONTEXT.md` (this file)
2. `ARCHITECTURE.md` — component responsibilities and data flow
3. `FILE_GUIDE.md` — where to find/change a specific thing
4. `CLAUDE.md` (project root) — authoritative crate table + commands, kept up
   to date by convention
5. `docs/SPEC.md` — functional spec, referenced by code comments (e.g. `SEC-03`)
6. `docs/PLAN.md` — PR-by-PR status; check before assuming a feature is "not
   built yet"

## Design principles to respect

- **`SemanticState` is the contract.** Agents never see raw DOM/HTML. Any
  change to `sre/` must keep `stable_key` identity stable across re-renders
  (tests: `core-runtime/tests/stable_key_*`, `sre_determinism.rs`).
- **Audit before execution, policy before mutation.** In `act`, the audit log
  entry is written *before* policy enforcement, and policy enforcement runs
  *before* any CDP mutation. Don't reorder this (see `core-runtime/src/browser.rs`
  around `enforce_policy`).
- **Capture state before propagating errors.** Don't let `?` silently drop
  accumulated metering/audit/speculative state — see CLAUDE.md §12.
- **No `std::env::set_var`/`remove_var` in tests** — inject env values as
  function parameters instead (CLAUDE.md §11).
- **Immutability by default** — return new values, don't mutate in place
  (workspace-wide Rust convention).

## Frequently changed areas

- `mcp-server/src/lib.rs` — tool dispatch, usage metering, speculative wiring
  (this file is large; expect most "new tool behavior" work to land here).
- `core-runtime/src/sre/` — semantic capture/normalization tuning.
- `core-runtime/src/policy.rs` — new policy rule shapes / Guardian Angel
  thresholds.
- `docs/PLAN.md` — updated every PR to track phase status.

## High blast-radius areas (change with care, check tests first)

- `core-runtime/src/sre/stable_key.rs` — identity hashing; a change here
  silently breaks every agent's ability to re-target elements across page
  re-renders.
- `core-runtime/src/browser.rs` — `enforce_policy`/audit ordering in `act()`
  and crash/disconnect recovery (`relaunch`, `is_browser_disconnected`).
- `core-runtime/src/speculative/` — feeds `get_state`'s fast path; a bug here
  causes agents to silently see stale state (`StateDelta::Mismatch` is the
  safety net — don't remove it).
- `core-runtime/src/prompt_injection.rs` — security-relevant; changes affect
  `security_flags` and `Redact` mode page-text mutation.
- `.config/nextest.toml` — CI's `[profile.ci]` does **not** inherit from
  `[profile.default]`; every field must be repeated (CLAUDE.md §7).
- `nfr-baseline/*.json` — only update via `scripts/update_nfr_baseline.sh`,
  never hand-edit.

## Checklist before starting work

- [ ] Read `AI_CONTEXT.md`, `ARCHITECTURE.md`, and the relevant section of
      `FILE_GUIDE.md` for the area you're touching.
- [ ] Check `docs/PLAN.md` for existing status/PR history on this feature.
- [ ] Identify the exact test files that exercise this code path
      (`FILE_GUIDE.md` lists test directories per crate).
- [ ] If touching `sre/`, `policy.rs`, or `browser.rs`'s `act`/`enforce_policy`
      path, re-read the relevant "High blast-radius" note above.
- [ ] Run `just check` and the targeted test file locally before considering
      the change done; run `just test-all` before a PR.
