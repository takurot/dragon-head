# Code Review

Thanks for adding the MCP tool interface! The implementation correctly exposes the tools and handles JSON-RPC 2.0. The schema definitions are also well-aligned with the specification. I have a few suggestions to improve robustness and compliance.

### 🔴 [important] Logic: Prevent Panic in `fallback_alias`

In `fallback_alias`, if the `stable_key` is somehow shorter than 8 characters, the slicing operation `&stable_key[..8]` will panic.
```rust
    if node.backend_node_id > 0 {
        format!("{}_{}", role, node.backend_node_id)
    } else {
        format!("{}_{}", role, &stable_key[..8]) // <-- Panic risk
    }
```
**Suggestion:** Ensure we take at most 8 characters safely.
```rust
    } else {
        let key_prefix: String = stable_key.chars().take(8).collect();
        format!("{}_{}", role, key_prefix)
    }
```

### 🟡 [important] Compliance: Strict JSON-RPC Error Codes

Currently, `handle_jsonrpc` maps all errors during method routing to `-32000` (Server Error). To fully comply with the JSON-RPC 2.0 specification, we should return the standard error codes for specific scenarios:
- **-32601 (Method not found)** for unsupported methods.
- **-32602 (Invalid params)** for missing or invalid `name` in `tools/call`.

**Suggestion:** Update the error handling in `handle_jsonrpc` to use the correct JSON-RPC codes.

### 🟢 [nit] Style: Unnecessary `format!`

In `render_state_markdown`:
```rust
    let mut lines = vec![
        format!("# Semantic State"),
```
`"# Semantic State".to_string()` is preferred over an empty `format!()` macro.

I will go ahead and apply these changes, ensure all tests and linters pass, and push the updates!