use anyhow::Result;
use json_patch::diff;
use mcp_server::{McpBackend, McpServer};
use serde_json::{json, Value};

#[derive(Default)]
struct MockBackend;

impl McpBackend for MockBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "get_state"}))
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "act"}))
    }

    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "verify"}))
    }

    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "get_visual"}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "ask_human"}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"ok": true, "tool": "run_skill"}))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }
}

/// A backend that returns controlled semantic state payloads for delta testing.
/// Handles delta delivery by tracking previous state and computing RFC 6902 patches.
struct SemanticMockBackend {
    states: Vec<Value>,
    call_count: usize,
    previous_state: Option<Value>,
}

impl SemanticMockBackend {
    fn with_states(states: Vec<Value>) -> Self {
        Self {
            states,
            call_count: 0,
            previous_state: None,
        }
    }
}

impl McpBackend for SemanticMockBackend {
    fn get_state(&mut self, arguments: Value) -> Result<Value> {
        let is_delta = arguments.get("delivery").and_then(|v| v.as_str()) == Some("delta");

        let idx = self.call_count.min(self.states.len().saturating_sub(1));
        self.call_count += 1;
        let current = self.states[idx].clone();

        if !is_delta {
            // Full delivery: return state and seed previous for future delta calls.
            self.previous_state = Some(current.clone());
            return Ok(current);
        }

        // Delta delivery: compute diff relative to previous state.
        let current_hash = current["metadata"]["state_hash"]
            .as_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|| format!("hash-{}", self.call_count));

        let response = match self.previous_state.take() {
            None => {
                self.previous_state = Some(current.clone());
                json!({ "type": "full", "hash": current_hash, "state": current })
            }
            Some(prev) => {
                let base_hash = prev["metadata"]["state_hash"]
                    .as_str()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| "prev-hash".to_string());

                if base_hash == current_hash {
                    self.previous_state = Some(current);
                    json!({ "type": "no_change", "hash": current_hash })
                } else {
                    let patch = diff(&prev, &current);
                    let patch_value = serde_json::to_value(&patch).unwrap();
                    self.previous_state = Some(current);
                    json!({
                        "type": "delta",
                        "base_hash": base_hash,
                        "next_hash": current_hash,
                        "patch": patch_value
                    })
                }
            }
        };
        Ok(response)
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "ok"}))
    }

    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"matched": true}))
    }

    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"image_sha256": "abc"}))
    }

    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"approved": true}))
    }

    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed"}))
    }

    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }
}

#[test]
fn test_mcp_contract_exposes_required_tools() {
    let server = McpServer::new(MockBackend);
    let tools = server.tools();
    let names: Vec<&str> = tools.iter().map(|tool| tool.name.as_str()).collect();
    let get_state = tools
        .iter()
        .find(|tool| tool.name == "get_state")
        .expect("get_state tool must exist");

    assert!(names.contains(&"get_state"));
    assert!(names.contains(&"act"));
    assert!(names.contains(&"verify"));
    assert!(names.contains(&"get_visual"));
    assert!(names.contains(&"ask_human"));
    assert!(names.contains(&"run_skill"));
    assert!(names.contains(&"get_usage_report"));
    assert_eq!(
        get_state.input_schema["properties"]["delivery"]["enum"],
        json!(["full", "delta"])
    );
}

#[test]
fn test_mcp_contract_all_tools_are_callable() {
    let mut server = McpServer::new(MockBackend);

    for name in [
        "get_state",
        "act",
        "verify",
        "get_visual",
        "ask_human",
        "run_skill",
    ] {
        let result = server
            .call_tool(name, json!({}))
            .unwrap_or_else(|_| panic!("tool call failed for {name}"));
        assert_eq!(result["ok"], json!(true));
        assert_eq!(result["tool"], json!(name));
    }

    // extract requires rule_name or inline — test with inline DSL
    let extract_result = server
        .call_tool("extract", json!({"inline": {"selector": "h1"}}))
        .expect("extract tool call failed");
    assert_eq!(extract_result["rule"], json!("mock"));

    let usage_report = server
        .call_tool("get_usage_report", json!({}))
        .expect("usage report tool call failed");
    assert_eq!(usage_report["plan_tier"], json!("enterprise"));
}

// ── Delta delivery tests ──────────────────────────────────────────────────────

fn make_full_state(url: &str) -> Value {
    // state_hash encodes the URL so different pages have distinct hashes
    let state_hash = format!("hash-{}", url.replace("://", "-").replace('/', "-"));
    json!({
        "metadata": {
            "url": url,
            "page_instance_id": "pid-1",
            "state_hash": state_hash,
            "load_profile": "interactive",
            "timestamp": 1_000_000u64
        },
        "interactive_elements": [
            {
                "id": 1,
                "stable_key": "btn-submit",
                "alias": "btn_1",
                "role": "button",
                "name": "Submit",
                "attributes": {},
                "bbox": [0.0, 0.0, 100.0, 50.0],
                "policy_flags": []
            }
        ]
    })
}

/// First call with delta delivery when no previous state exists returns full state
/// with a `no_previous_state` hint.
#[test]
fn test_delta_first_call_returns_full_state_with_hint() -> Result<()> {
    use mcp_server::PlanTier;

    let backend = SemanticMockBackend::with_states(vec![make_full_state("https://example.com")]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;

    // Should return full state tagged as "full" with a hint
    assert_eq!(
        result["type"],
        json!("full"),
        "first delta call must return type=full"
    );
    assert!(
        result["hash"].is_string(),
        "first delta call must include hash"
    );
    assert!(
        result["state"].is_object(),
        "first delta call must include state object"
    );

    Ok(())
}

/// Second call with same state returns no_change response.
#[test]
fn test_delta_no_change_returns_no_change() -> Result<()> {
    use mcp_server::PlanTier;

    let state = make_full_state("https://example.com");
    let backend = SemanticMockBackend::with_states(vec![state.clone(), state.clone()]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    // Seed the previous state with a full call
    server.call_tool("get_state", json!({"delivery": "delta"}))?;

    // Second call with same state
    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;

    assert_eq!(
        result["type"],
        json!("no_change"),
        "unchanged state must return type=no_change"
    );
    assert!(
        result["hash"].is_string(),
        "no_change response must include hash"
    );

    Ok(())
}

/// Second call with changed state returns proper RFC 6902 delta.
#[test]
fn test_delta_changed_state_returns_patch() -> Result<()> {
    use mcp_server::PlanTier;

    let state_v1 = make_full_state("https://example.com/page1");
    let state_v2 = make_full_state("https://example.com/page2");
    let backend = SemanticMockBackend::with_states(vec![state_v1, state_v2]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    // Seed previous state
    server.call_tool("get_state", json!({"delivery": "delta"}))?;

    // Delta call with changed state
    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;

    assert_eq!(
        result["type"],
        json!("delta"),
        "changed state must return type=delta"
    );
    assert!(
        result["base_hash"].is_string(),
        "delta response must include base_hash"
    );
    assert!(
        result["next_hash"].is_string(),
        "delta response must include next_hash"
    );
    assert_ne!(
        result["base_hash"], result["next_hash"],
        "hashes must differ when state changed"
    );
    assert!(
        result["patch"].is_array(),
        "delta response must include patch array"
    );

    // The patch should have at least one operation (URL changed)
    let patch = result["patch"].as_array().expect("patch must be array");
    assert!(
        !patch.is_empty(),
        "patch must not be empty when state changed"
    );

    // Verify each patch operation has op and path fields
    for op in patch {
        assert!(op["op"].is_string(), "each patch op must have op field");
        assert!(op["path"].is_string(), "each patch op must have path field");
    }

    Ok(())
}

/// Delta via JSON-RPC path also works correctly.
#[test]
fn test_delta_via_jsonrpc_returns_correct_shape() -> Result<()> {
    use mcp_server::PlanTier;

    let state_v1 = make_full_state("https://example.com/page1");
    let state_v2 = make_full_state("https://example.com/page2");
    let backend = SemanticMockBackend::with_states(vec![state_v1, state_v2]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    // Seed via JSON-RPC
    let seed_req = json!({
        "jsonrpc": "2.0",
        "id": 1,
        "method": "tools/call",
        "params": {
            "name": "get_state",
            "arguments": {"delivery": "delta"}
        }
    });
    server.handle_jsonrpc(&serde_json::to_string(&seed_req).unwrap());

    // Delta call via JSON-RPC
    let delta_req = json!({
        "jsonrpc": "2.0",
        "id": 2,
        "method": "tools/call",
        "params": {
            "name": "get_state",
            "arguments": {"delivery": "delta"}
        }
    });
    let response_str = server
        .handle_jsonrpc(&serde_json::to_string(&delta_req).unwrap())
        .expect("response must be present");
    let response: Value = serde_json::from_str(&response_str).unwrap();

    assert!(response["error"].is_null(), "no error expected");
    let payload = &response["result"]["content"][0]["json"];
    assert_eq!(payload["type"], json!("delta"));

    Ok(())
}

/// Full delivery mode is unaffected by delta state tracking.
#[test]
fn test_full_delivery_unaffected_by_delta_state() -> Result<()> {
    use mcp_server::PlanTier;

    let state = make_full_state("https://example.com");
    let backend = SemanticMockBackend::with_states(vec![state.clone(), state.clone()]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Enterprise);

    // Full delivery should return the raw state (no type/hash envelope)
    let result = server.call_tool("get_state", json!({"delivery": "full"}))?;

    // Full mode should return raw state as-is (metadata + interactive_elements)
    assert!(
        result["metadata"].is_object(),
        "full delivery must return metadata"
    );
    assert!(
        result["interactive_elements"].is_array(),
        "full delivery must return interactive_elements"
    );

    Ok(())
}

/// full delivery followed by delta delivery must produce no_change (not a spurious patch).
/// This verifies the fix for the previous_state update timing bug: full delivery must update
/// previous_state so the next delta request compares against the most recent state.
#[test]
fn test_full_delivery_seeds_previous_state_for_subsequent_delta() -> Result<()> {
    use mcp_server::PlanTier;

    let state = make_full_state("https://example.com");
    // Both calls return the identical state
    let backend = SemanticMockBackend::with_states(vec![state.clone(), state.clone()]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    // Seed previous_state via full delivery
    server.call_tool("get_state", json!({"delivery": "full"}))?;

    // Delta call should see no change relative to the state seeded by full delivery
    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;
    assert_eq!(
        result["type"],
        json!("no_change"),
        "delta after full delivery of same state must return no_change, not full"
    );

    Ok(())
}

// ── security_flags in get_state output ───────────────────────────────────────

/// Backend that returns a state containing an element with security_flags set.
struct SecurityFlagsMockBackend {
    include_flags: bool,
}

impl McpBackend for SecurityFlagsMockBackend {
    fn get_state(&mut self, _arguments: Value) -> Result<Value> {
        let mut element = json!({
            "id": 1,
            "stable_key": "key-btn-flagged",
            "alias": "btn_flagged",
            "role": "button",
            "name": "Click",
            "attributes": {},
            "bbox": [0.0, 0.0, 100.0, 50.0],
            "policy_flags": []
        });
        if self.include_flags {
            element["security_flags"] = json!(["possible_prompt_injection"]);
        }

        Ok(json!({
            "metadata": {
                "url": "https://example.com",
                "page_instance_id": "pid-1",
                "state_hash": "hash-abc",
                "load_profile": "interactive",
                "timestamp": 1_000_000u64
            },
            "interactive_elements": [element]
        }))
    }

    fn act(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "ok"}))
    }
    fn verify(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"matched": true}))
    }
    fn get_visual(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"image_sha256": "abc"}))
    }
    fn ask_human(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"approved": true}))
    }
    fn run_skill(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"status": "completed"}))
    }
    fn extract(&mut self, _arguments: Value) -> Result<Value> {
        Ok(json!({"rule": "mock", "result": null}))
    }
}

/// get_state JSON output includes security_flags when the element is flagged.
#[test]
fn test_get_state_json_includes_security_flags_for_flagged_element() -> Result<()> {
    let mut server = McpServer::new(SecurityFlagsMockBackend {
        include_flags: true,
    });
    let result = server.call_tool("get_state", json!({}))?;

    let elements = result["interactive_elements"]
        .as_array()
        .expect("interactive_elements must be array");
    assert_eq!(elements.len(), 1);

    let flags = &elements[0]["security_flags"];
    assert!(
        flags.is_array(),
        "security_flags must be array when element is flagged: {result}"
    );
    assert_eq!(
        flags[0],
        json!("possible_prompt_injection"),
        "security_flags must contain the expected flag"
    );

    Ok(())
}

/// get_state JSON output omits security_flags entirely for clean (unflagged) elements.
/// This ensures backward compatibility with clients that don't expect the field.
#[test]
fn test_get_state_json_omits_security_flags_for_clean_element() -> Result<()> {
    let mut server = McpServer::new(SecurityFlagsMockBackend {
        include_flags: false,
    });
    let result = server.call_tool("get_state", json!({}))?;

    let elements = result["interactive_elements"]
        .as_array()
        .expect("interactive_elements must be array");
    assert_eq!(elements.len(), 1);

    assert!(
        elements[0].get("security_flags").is_none(),
        "security_flags must be omitted when element has no flags (backward compat): {result}"
    );

    Ok(())
}

// NOTE: Markdown rendering tests (security_flags in markdown output, sanitization of
// malicious flag characters, multi-flag comma-join) live in mcp-server/src/lib.rs unit
// tests for render_state_markdown (render_state_markdown_includes_security_flags_when_present,
// render_state_markdown_omits_security_flags_line_when_empty, etc.).  A mock-based test here
// would only exercise the mock's own output, not render_state_markdown itself.

/// policy_flags and security_flags remain separate in JSON output.
#[test]
fn test_get_state_json_keeps_policy_and_security_flags_separate() -> Result<()> {
    let mut server = McpServer::new(SecurityFlagsMockBackend {
        include_flags: true,
    });
    let result = server.call_tool("get_state", json!({}))?;

    let elem = &result["interactive_elements"][0];
    assert_eq!(
        elem["policy_flags"],
        json!([]),
        "policy_flags must be empty for this element"
    );
    assert_eq!(
        elem["security_flags"],
        json!(["possible_prompt_injection"]),
        "security_flags must carry the expected flag"
    );
    // Verify they are stored under distinct keys in the JSON object.
    assert!(
        elem.as_object()
            .map(|o| o.contains_key("policy_flags") && o.contains_key("security_flags"))
            .unwrap_or(false),
        "both policy_flags and security_flags must be present as separate top-level keys"
    );

    Ok(())
}

/// full delivery followed by delta delivery with a changed state must return a proper patch.
#[test]
fn test_full_then_delta_with_changed_state_returns_patch() -> Result<()> {
    use mcp_server::PlanTier;

    let state_v1 = make_full_state("https://example.com/page1");
    let state_v2 = make_full_state("https://example.com/page2");
    // First call (full) → state_v1; second call (delta) → state_v2
    let backend = SemanticMockBackend::with_states(vec![state_v1, state_v2]);
    let mut server = McpServer::new_with_plan(backend, PlanTier::Pro);

    // Seed previous_state via full delivery
    server.call_tool("get_state", json!({"delivery": "full"}))?;

    // Delta call should detect the change and return a patch
    let result = server.call_tool("get_state", json!({"delivery": "delta"}))?;
    assert_eq!(
        result["type"],
        json!("delta"),
        "delta after full delivery of different state must return type=delta"
    );
    assert!(
        result["patch"].is_array(),
        "delta response must include patch array"
    );

    Ok(())
}
