# Dragon Head — Developer Examples

This directory contains runnable examples and reference JSON that let a new
contributor understand Dragon Head's core concepts **without a running Chrome
instance or any paid credentials**.

---

## Prerequisites

| Requirement | Version |
|-------------|---------|
| Rust (stable) | 1.75+ |
| Chrome/Chromium | Only needed for **browser integration** tests (not these examples) |

Install Rust via [rustup](https://rustup.rs/):

```bash
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
```

---

## Quick Start

```bash
git clone <repo-url>
cd dragon-head
cargo run --example quickstart
```

Expected output:

```
=== Dragon Head Quickstart ===

[1] SemanticState constructed
    page_instance_id : <uuid>
    state_hash       : <sha256-hex>
    load_profile     : Minimal

[2] Fast State generated
    interactive_elements : 2
    messages             : 1
      → role=input     alias=input_email               stable_key=b2c3d4e5...
      → role=button    alias=btn_purchase              stable_key=a1b2c3d4...

[3] PolicyEngine loaded with 1 rule(s)
    safe.example.com       → Allow
    blocked-domain.example.com → Block

[4] MCP get_state response (JSON):
{
  "metadata": { ... },
  "interactive_elements": [ ... ]
}

=== Done — no credentials required ===
```

---

## Examples

### `quickstart.rs` — Core concepts in one file

```bash
cargo run --example quickstart
```

Demonstrates:

1. **SemanticState** — build the AI-readable page representation from Rust structs
2. **Fast State** — extract interactive elements and messages (SPEC SRE-01)
3. **PolicyEngine** — evaluate a domain-block rule against a PolicyContext
4. **MCP payload** — serialize state as a `get_state` JSON response

### `policy_cookbook.rs` — PolicyRule recipes

```bash
cargo run --example policy_cookbook
```

Shows four common policy patterns:

| Recipe | Description |
|--------|-------------|
| Block domain | Prevent navigation to a restricted hostname |
| Require approval (financial) | HITL gate on purchase buttons when price context is detected |
| Time-boxed approval | 5-minute approval window on `/checkout` path |
| Load from JSON | Parse rules from a JSON string (same format as `sample_policy.json`) |

---

## Reference Files

### `sample_policy.json`

A realistic policy rule set you can load with `PolicyEngine::try_from_file`:

```rust
let engine = PolicyEngine::try_from_file("examples/sample_policy.json")?;
```

Rules included:

| Rule ID | Effect |
|---------|--------|
| `block-navigation-to-restricted-domain` | Blocks all access to `blocked-domain.example.com` |
| `block-account-destruction` | Blocks buttons matching "delete/remove/close account" |
| `require-approval-financial-action` | Human approval for payment buttons with price context |
| `require-approval-payments-checkout-path` | Approval until navigation on `payments.example.com/checkout` |

### `sample_skill.json`

A declarative **Skill** definition for an end-to-end checkout workflow.
Skills are executed by the Skills Engine (Layer 3). Steps follow the
`verify → policy_check → act → post_check` order enforced by the SPEC.

```json
{
  "schema_version": 1,
  "name": "checkout",
  "steps": [
    { "type": "locate", ... },
    { "type": "verify", ... },
    { "type": "act",    "action": "type",  "target": "id:43", "value": "{{email}}" },
    { "type": "act",    "action": "click", "target": "id:42" },
    { "type": "wait",   "condition": "intent:checkout_complete", ... },
    { "type": "extract","key": "order_id", ... }
  ]
}
```

---

## MCP Tool Contract Examples

The `mcp_examples/` directory contains paired request/response JSON files that
document every MCP tool call your LLM or integration layer will make.
Successful responses place the JSON object in `result.structuredContent` and
repeat its serialized form in a `text` content block for client fallback.

| File pair | Tool | Scenario |
|-----------|------|----------|
| `get_state_request.json` / `get_state_response.json` | `get_state` | Full semantic state delivery |
| `get_state_delta_request.json` / `get_state_delta_first_response.json` | `get_state` | Delta delivery — first call returns full state (`type: "full"`) |
| `get_state_delta_request.json` / `get_state_delta_response.json` | `get_state` | Delta delivery — changed state (`type: "patch"`, RFC 6902) — requires Pro plan |
| `get_state_delta_request.json` / `get_state_delta_no_change_response.json` | `get_state` | Delta delivery — hashes match, no state change (`type: "no_change"`) |
| `act_request.json` / `act_response.json` | `act` | Successful click action |
| `act_request.json` / `act_blocked_response.json` | `act` | Action blocked by policy (requires human approval) |
| `verify_request.json` / `verify_response.json` | `verify` | Text precondition matched |
| `verify_request.json` / `verify_mismatch_response.json` | `verify` | Text mismatch (anti-hallucination check) |

### `get_state` (full delivery)

**Request:**
```json
{
  "method": "tools/call",
  "params": {
    "name": "get_state",
    "arguments": { "format": "json", "delivery": "full" }
  }
}
```

**Response** (excerpt):
```json
{
  "metadata": {
    "url": "https://example.com/checkout",
    "state_hash": "a1b2c3d4...",
    "load_profile": "interactive"
  },
  "interactive_elements": [
    {
      "id": 42,
      "stable_key": "a1b2c3d4...",
      "alias": "btn_purchase",
      "role": "button",
      "name": "Purchase",
      "policy_flags": ["financial_transaction"]
    }
  ]
}
```

### `get_state` (delta delivery)

When `delivery: "delta"` is set (Pro plan), the server returns one of three
response shapes depending on the call context:

**First call** — no prior hash on record; the server returns the full state:
```json
{
  "type": "full",
  "state_hash": "a1b2c3d4...",
  "metadata": { "..." },
  "interactive_elements": [ "..." ]
}
```

**Subsequent call — state changed** — the server returns an RFC 6902 JSON Patch,
reducing token consumption by up to 95 %:
```json
{
  "type": "patch",
  "base_hash": "a1b2c3d4...",
  "next_hash": "c3d4e5f6...",
  "patch": [
    { "op": "replace", "path": "/interactive_elements/0/attributes/disabled", "value": true }
  ]
}
```

**Subsequent call — no change** — the client-side hash matches; the server
returns a lightweight sentinel so the agent can skip re-processing:
```json
{
  "type": "no_change",
  "state_hash": "a1b2c3d4..."
}
```

### `act`

```json
{ "action": "click", "target_id": 42, "target_stable_key": "a1b2c3d4..." }
```

Possible response statuses: `ok`, `verify_required`, `blocked`,
`requires_human_approval`.

### `verify`

Always call `verify` before `act` on high-stakes elements to confirm the page
has not changed between `get_state` and the action:

```json
{ "target_id": 42, "expected": { "text": "Purchase" } }
```

---

## Architecture Primer

```
LLM / Agent
    │
    │  JSON-RPC (MCP)
    ▼
McpServer  ──────────────────────── UsageMeters / PlanGate
    │
    │  McpBackend trait
    ▼
CoreRuntimeBackend
    ├── PageSession ──── Chrome CDP ──── Browser
    ├── PolicyEngine ─── PolicyRule[]
    ├── SkillEngine ──── SkillDefinition[]
    └── AuditLog
```

Key types (all in `core-runtime`):

| Type | Purpose |
|------|---------|
| `SemanticState` | Deterministic AI-readable page snapshot |
| `SemanticNode` | One element in the semantic tree |
| `FastSemanticState` | `interactive_elements` + `messages` (< 50 ms) |
| `PolicyEngine` | Evaluates `PolicyRule[]` against a `PolicyContext` |
| `PolicyDecision` | `allow` / `block` / `require_human_approval` + scope |
| `SemanticDelta` | RFC 6902 patch between two states |
