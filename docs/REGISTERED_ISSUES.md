# Registered Issues

This document tracks identified issues and areas for improvement in Dragon Head.

## [ISSUE-01] MCP `get_state` Delta implementation missing

### Description
The MCP `get_state` tool supports a `delivery: "delta"` argument as per specification, but the current implementation in `mcp-server/src/lib.rs` ignores this argument and always returns the full `ExternalSemanticState`.

### Impact
- Higher bandwidth usage for incremental updates.
- Divergence from the specification (SRE-02).

### Task
- [ ] Update `CoreRuntimeBackend::get_state` to handle `StateDelivery::Delta`.
- [ ] Implement conversion from `core_runtime::sre::StateUpdate` to MCP-compatible delta payload.

---

## [ISSUE-02] `PolicyEngine` path prefix matching is not segment-aware

### Description
`PolicyEngine` uses `starts_with` for path prefix matching without ensuring segment boundaries. For example, a rule for `/login` will incorrectly match `/logina`.

### Reproduction
Validated in `core-runtime/tests/repro_policy_bug.rs`.

### Task
- [ ] Update `CompiledPolicyRule::matches` in `core-runtime/src/policy.rs` to ensure segment-aware path matching (e.g., using exact match or ensuring next char is `/`).

---

## [ISSUE-03] Hardcoded viewport constants in quadrant calculation

### Description
`core-runtime/src/sre/normalization.rs` uses hardcoded `DEFAULT_VIEWPORT_WIDTH_PX` (800) and `DEFAULT_VIEWPORT_HEIGHT_PX` (600) for calculating quadrants. This leads to incorrect quadrant assignment when the actual browser viewport differs.

### Impact
- `stable_key` instability or incorrect differentiation if elements move across the hardcoded 400/300 boundaries but not the actual viewport center.

### Task
- [ ] Allow passing actual viewport dimensions to `normalize_dom`.
- [ ] Update `PageSession` to provide current viewport size during semantic state capture.

---

## [ISSUE-04] `mcp-server` comprehensive evaluation test compilation instability

### Description
Evidence of compilation failure in `mcp-server/tests/comprehensive_evaluation.rs` due to `should_skip` vs `should_skip_browser_tests` mismatch. Although it recently passed, the inconsistency in helper function names across crates indicates a need for refactoring.

### Task
- [ ] Standardize browser test skip helper names across the workspace.
- [ ] Move shared test helpers to `test-bench-support` or a dedicated internal crate.

---

## [ISSUE-05] `plugin-host` missing execution entry point

### Description
`plugin-host` crate provides manifest validation and signature verification but lacks a high-level API to execute the Wasm modules for the defined extension points (`on_state`, `before_act`).

### Task
- [ ] Implement `PluginRuntime` or similar to handle `wasmtime` instantiation and call exports.
- [ ] Integrate plugin execution into the `core-runtime` pipeline.
