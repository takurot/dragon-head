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

---

## [ISSUE-06] `McpServer` usage metering gaps

### Description
The `McpServer` only records usage for `get_state`, `get_visual`, and `act`. Calls to `ask_human` and the internal operations performed during `run_skill` (e.g., `act` or `get_visual` calls within a skill workflow) are not being metered.

### Impact
- Inaccurate billing and usage reporting.
- Under-counting of value-based events (Section 7.1).

### Task
- [ ] Update `McpServer::record_usage` to handle `ask_human` tool calls.
- [ ] Implement a mechanism for `PageSkillRuntime` to report metered operations back to `McpServer` during `run_skill` execution.

---

## [ISSUE-07] `SkillsEngine` integration NO-OPs and limitations

### Description
The `PageSkillRuntime` (the bridge between Skills Engine and Core Runtime) has placeholder or limited implementations for several steps. `locate` is a NO-OP (always returns `Success`), and `wait` only supports `intent:` prefixed conditions.

### Impact
- Skills relying on element location verification or complex wait conditions (e.g., waiting for an element to be enabled without a specific intent marker) may succeed prematurely or fail to detect errors.

### Task
- [ ] Implement `PageSkillRuntime::locate` to verify element existence in the current DOM/SRE.
- [ ] Expand `PageSkillRuntime::wait` to support semantic state wait (e.g., `id:123:enabled`) by calling `page.wait_for_semantic`.

---

## [ISSUE-08] Inconsistent PII masking between SRE and Audit Logs

### Description
`core-runtime/src/sre/normalization.rs` only masks credit card numbers in text nodes, whereas `core-runtime/src/audit.rs` masks both credit cards and email addresses.

### Impact
- Potential PII leakage in the `SemanticState` JSON if email addresses are present in text nodes.
- Inconsistency in security posture across different system layers.

### Task
- [ ] Standardize PII masking regexes and logic into a shared utility within `core-runtime`.
- [ ] Apply consistent masking to both SRE text extraction and Audit Log sanitization.

---

## [ISSUE-09] `AuditLogger` lacks persistent storage implementation

### Description
The current `AuditLogger` in `core-runtime/src/audit.rs` only maintains an in-memory buffer (512 recent events) and optionally prints to stdout. There is no implementation for persistent file storage or SIEM integration.

### Impact
- Loss of audit trail on process restart or buffer overflow.
- Failure to meet enterprise compliance requirements for long-term audit retention.

### Task
- [ ] Implement a persistent sink for `AuditLogger` (e.g., rolling file logs).
- [ ] Ensure that `audit_retention_snapshot` in `McpServer` can reflect persistent storage metrics.
