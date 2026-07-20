# FILE_GUIDE.md

## Directory Overview

| Path | Role | Change frequency | Notes |
|---|---|---:|---|
| `core-runtime/src/` | Chrome/CDP session, Semantic State, policy, audit, privacy, plugins, speculative state | High | `stable_key.rs` and `browser.rs` have broad impact |
| `core-runtime/tests/` | Core-runtime integration tests | High | Check matching tests for behavior changes |
| `mcp-server/src/` | `dragon-head-mcp` binary, MCP tool dispatch, config loading | High | Most tool behavior is centralized in `lib.rs` |
| `mcp-server/tests/` | MCP protocol and contract tests | Medium | Update when tool schemas or behavior change |
| `skills-engine/src/` | Declarative workflow definitions and execution | Low-Medium | `lib.rs` is the main implementation file |
| `plugin-host/src/` | Wasm plugin validation and execution | Low | Signature and SBOM validation are security-sensitive |
| `marketplace/src/` | Plugin/domain-pack metadata | Low | Keep marketplace docs in sync when metadata changes |
| `hitl-bridge/src/` | Slack/Teams HITL bridge | Low | `lock.rs` and `server.rs` cover HMAC and double-resolution safety |
| `bench/src/` | NFR/ROI benchmark harness | Low | Keep baselines script-driven |
| `test-bench-support/src/` | Test helpers for Chrome detection | Low | Used by browser-dependent tests |
| `examples/` | Chrome-free examples and MCP request/response fixtures | Low | Keep README examples in sync |
| `docs/` | Specification, architecture, operations, and agent guidance | Medium | Verify implementation claims against code |
| `scripts/` | Installer, evaluation, NFR, and audit helper scripts | Low | Prefer scripts over manual data edits |
| `nfr-baseline/*.json` | NFR regression baselines | Low | Update only through `scripts/update_nfr_baseline.sh` |
| `.github/workflows/` | CI/CD definitions | Low | Keep in sync with `deny.toml` and nextest profiles |

## Important Files

### `core-runtime/src/`

- `lib.rs` — crate root and public API exports.
- `browser.rs` — `BrowserClient`, `PageSession`, action execution, policy, audit, and recovery.
- `chrome_detection.rs` — Chrome/Chromium discovery.
- `dom_signature.rs` — fallback element matching.
- `policy.rs` — `PolicyEngine`, `PolicyRule`, `PolicyDecision`, `OutcomeProjection`.
- `privacy.rs` — PII detection and redaction.
- `prompt_injection.rs` — prompt-injection detection and redaction.
- `audit.rs`, `audit_sink.rs`, `audit_replay.rs` — audit event model, sinks, and replay.
- `session_vault.rs` — encrypted cookie/session storage.
- `plugin_hooks.rs` — plugin hook boundary.
- `speculative/{mod,model,codec}.rs` — speculative state generation.
- `sre/{state,normalization,pipeline,profile,stable_key}.rs` — Semantic Rendering Engine.

### `mcp-server/src/`

- `main.rs` — binary entrypoint and stdio loop.
- `lib.rs` — MCP tools, dispatch, metering, and backend wiring.
- `config.rs` — `config.toml` and environment override loading.
- `doctor.rs` — `--doctor` checks.
- `init.rs` — `--init` MCP client snippets.
- `cli.rs` — CLI argument parsing.

### Other Crates

- `skills-engine/src/lib.rs` — `SkillDefinition`, `SkillStep`, `SkillEngine`, `SkillRuntime`.
- `plugin-host/src/lib.rs` — `PluginManifest`, `PluginRuntime`, Wasm execution.
- `plugin-host/src/schema_registry.rs` — extraction rule registry.
- `hitl-bridge/src/server.rs` — Slack interaction endpoint and HMAC verification.
- `hitl-bridge/src/bridge.rs` — gateway polling, notification, and resolution orchestration.
- `hitl-bridge/src/lock.rs` — double-resolution prevention.

## Feature-To-File Map

### MCP Tool Behavior

- `mcp-server/src/lib.rs`
- `mcp-server/tests/mcp_protocol_compliance.rs`
- `mcp-server/tests/mcp_client_contract.rs`
- `mcp-server/tests/mcp_schema_compatibility.rs`
- `README.md` Available MCP Tools table
- `docs/ARCHITECTURE.md`
- `docs/AI_CONTEXT.md`

### Semantic State / DOM Normalization

- `core-runtime/src/sre/normalization.rs`
- `core-runtime/src/sre/pipeline.rs`
- `core-runtime/src/sre/state.rs`
- `core-runtime/tests/sre_determinism.rs`
- `core-runtime/tests/sre_snapshot_regression.rs`
- `core-runtime/tests/sre_fast_full_state.rs`
- `core-runtime/tests/fixtures/golden/*.json`

### Stable Keys

- `core-runtime/src/sre/stable_key.rs`
- `core-runtime/tests/stable_key_*.rs`
- `core-runtime/tests/spa_stable_key_stress.rs`

### Policy / HITL

- `core-runtime/src/policy.rs`
- `core-runtime/tests/policy_engine.rs`
- `core-runtime/tests/policy_enforcement.rs`
- `core-runtime/tests/policy_schema_lint.rs`
- `hitl-bridge/src/*.rs`
- `mcp-server/tests/mcp_hitl_flow.rs`
- `docs/hitl-slack-bridge.md`

### Speculative State

- `core-runtime/src/speculative/{mod,model,codec}.rs`
- `core-runtime/tests/speculative_pregeneration.rs`
- `mcp-server/tests/speculative_get_state_ttft.rs`

### Audit / Privacy

- `core-runtime/src/audit.rs`
- `core-runtime/src/audit_sink.rs`
- `core-runtime/src/audit_replay.rs`
- `core-runtime/src/privacy.rs`
- `core-runtime/tests/audit_logging.rs`
- `core-runtime/tests/audit_persistence.rs`
- `core-runtime/tests/audit_schema.rs`
- `core-runtime/tests/pii_redaction.rs`
- `core-runtime/tests/pii_injection_composition.rs`

### Prompt Injection

- `core-runtime/src/prompt_injection.rs`
- `core-runtime/tests/prompt_injection_pipeline.rs`
- `mcp-server/tests/extract_prompt_injection.rs`
- `README.md` security and MCP tool sections

### Skills / Plugins

- `skills-engine/src/lib.rs`
- `skills-engine/tests/skill_conformance.rs`
- `skills-engine/tests/skill_schema_compatibility.rs`
- `plugin-host/src/lib.rs`
- `plugin-host/src/schema_registry.rs`
- `plugin-host/tests/plugin_signature_verification.rs`
- `plugin-host/tests/plugin_sbom_validation.rs`
- `core-runtime/src/plugin_hooks.rs`

### Browser Recovery

- `core-runtime/src/browser.rs`
- `core-runtime/tests/browser_recovery.rs`
- `core-runtime/tests/cdp_connectivity.rs`
- `mcp-server/tests/mcp_browser_recovery.rs`

### Metering / Billing

- `mcp-server/src/lib.rs`
- `mcp-server/tests/mcp_billing_plan_gating.rs`
- `mcp-server/tests/mcp_usage_metering_gaps.rs`
- `mcp-server/tests/mcp_pricing_snapshot.rs`

### NFR / Performance

- `bench/src/`
- `core-runtime/tests/nfr_*.rs`
- `nfr-baseline/*.json`
- `scripts/update_nfr_baseline.sh`
- `scripts/nfr_trend.py`
- `scripts/nfr_dashboard.py`

## Generated Or Sensitive Files

| Path | Type | Handling |
|---|---|---|
| `Cargo.lock` | Generated dependency lockfile | Do not edit manually |
| `target/**` | Build output | Do not commit |
| `core-runtime/target/nfr-dashboard*.md` | Benchmark output | Do not commit |
| `nfr-baseline/*.json` | Baseline data | Update only via script |
| `core-runtime/tests/fixtures/som/som_visual_baseline.png` | Binary fixture (plain git blob, below the ~500 KB LFS threshold — see `.gitattributes` exception) | Do not mix into docs-only PRs |
| `.config/nextest.toml` | nextest profiles | `ci` does not inherit from `default` |
| `deny.toml` | cargo-deny config | Add rationale for advisory exceptions |
