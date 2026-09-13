# Session Vault (ISSUE-210)

`core-runtime` includes a `SessionVault`/`KmsAdapter` subsystem (`core-runtime/src/session_vault.rs`)
for encrypting and persisting browser session state (currently cookies) under a session ID, and
`PageSession::save_to_vault`/`load_from_vault` methods to write/read it. As of this writing,
**`dragon-head-mcp` never calls either method** — no MCP tool or session-lifecycle hook wires
them up. Every `BrowserClient` still constructs a real `LocalSessionVault`/`SoftwareKms` pair
internally (it's required plumbing for the type to compile, not optional overhead), but nothing
in the shipped binary ever reads or writes through it.

## Why gated behind a feature instead of wired up or deleted

Per the tracked issue, this had two acceptable resolutions: wire it into the MCP surface, or gate
it until a real caller exists. Wiring it up well requires answering questions this issue didn't
scope — a session identifier scheme, where the agent-facing risk boundary sits (should an agent
be able to trigger save/load itself, or only an operator/lifecycle hook?), storage location and
retention, and how this interacts with browser-restart recovery (ISSUE-149) — rather than
inventing that design under an unrelated hygiene fix. Gating is the conservative, honest choice:
`save_to_vault`/`load_from_vault` are compiled and tested (see below) but require an explicit
opt-in, so they don't ship as unused, untested-in-production public surface while remaining
available to a future caller without deleting working code.

Gating was chosen over the issue's other named option, `#[doc(hidden)]`, because it's a stronger
guarantee: `#[doc(hidden)]` only hides the API from generated docs — the code still compiles and
ships unconditionally. The Cargo feature actually removes `save_to_vault`/`load_from_vault`/the
public `new_with_vault` from any build that doesn't opt in, at the cost of the small amount of
build-graph complexity documented below.

## Using it

```toml
# In a crate that depends on core-runtime and wants save_to_vault/load_from_vault:
core-runtime = { path = "...", features = ["session-vault-api"] }
```

`core-runtime`'s own test suite enables this feature for itself via a self-referential
`dev-dependency` (a standard Cargo idiom for test-only features), so `cargo test -p core-runtime`
already exercises both methods — no extra flags needed for the existing regression tests:
`core-runtime/tests/session_management.rs`, `core-runtime/tests/comprehensive_evaluation.rs`
(both call `BrowserClient::new_with_vault` directly), and the in-lib
`#[tokio::test] test_session_vault_save_load` in `core-runtime/src/browser/mod.rs`.

**Feature-unification caveat**: with Cargo's default resolver, a dev-dependency's requested
features are unified onto the single in-process `core-runtime` build for the whole invocation —
so `cargo test --workspace` and `cargo clippy --workspace --all-targets` (and the CI `coverage`
job, which uses `cargo llvm-cov nextest --workspace`) all compile **every** crate, including
`mcp-server`, against a `session-vault-api`-enabled `core-runtime`. The gate still holds where it
matters — `cargo build -p mcp-server --bin dragon-head-mcp --release` and the plain `cargo clippy
--workspace -- -D warnings` CI `lint` job both build/lint `core-runtime` without the feature — but
don't treat "it compiled under `cargo test --workspace`" as proof nothing in `mcp-server` calls
these methods; check the release/lint builds (or grep) for that.

## What a real caller would still need to design

- **Session ID scheme**: today's tests pass an arbitrary string; a production caller needs a
  stable, collision-resistant identifier (per-domain? per-MCP-client? per-operator-request?).
- **Trigger point**: an MCP tool the agent calls directly, an automatic save on a lifecycle event
  (e.g. before browser-restart recovery discards the old `PageSession`), or both.
- **Key management**: `BrowserClient::new()` today generates a fresh random `SoftwareKms` key
  every process start (see its doc comment) — a real caller needs a persistent key strategy, or
  saved sessions become unreadable across restarts.
- **Threat model**: whether an agent-facing tool should be able to read/write arbitrary session
  IDs, and what that implies for cross-session data exposure.

None of that is implemented here — this document exists so the next person wiring this up starts
from an explicit list of open questions instead of rediscovering them.
