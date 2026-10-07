use super::*;

pub(crate) fn parse_target_id(target: &str) -> Option<i64> {
    target
        .trim()
        .strip_prefix("id:")
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

pub(crate) fn parse_target_stable_key(target: &str) -> Option<String> {
    target
        .trim()
        .strip_prefix("stable_key:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

/// Parse a semantic wait condition of the form `id:<numeric_id>:<state>`.
/// Returns the target and desired state, or `None` for unrecognised formats.
pub(crate) fn parse_semantic_wait_condition(
    condition: &str,
) -> Option<(SemanticTarget, SemanticWaitState)> {
    let mut parts = condition.splitn(3, ':');
    let prefix = parts.next()?;
    let raw_id = parts.next()?.trim();
    let raw_state = parts.next()?.trim();
    if prefix != "id" {
        return None;
    }
    let id = raw_id.parse::<i64>().ok()?;
    let desired_state = match raw_state {
        "enabled" => SemanticWaitState::Enabled,
        _ => return None,
    };
    Some((SemanticTarget::Id(id), desired_state))
}

pub(crate) fn node_exists_by_id(node: &SemanticNode, target_id: i64) -> bool {
    if node.backend_node_id == target_id {
        return true;
    }
    node.children
        .iter()
        .any(|child| node_exists_by_id(child, target_id))
}

pub(crate) fn render_state_markdown(payload: &ExternalSemanticState) -> String {
    let mut lines = vec![
        "# Semantic State".to_string(),
        format!("- URL: {}", payload.metadata.url),
        format!("- Page Instance ID: {}", payload.metadata.page_instance_id),
        format!("- State Hash: {}", payload.metadata.state_hash),
        format!("- Load Profile: {}", payload.metadata.load_profile),
        format!("- Timestamp: {}", payload.metadata.timestamp),
        format!("- Speculative: {}", payload.metadata.speculative),
        String::new(),
        "## Interactive Elements".to_string(),
    ];

    for element in &payload.interactive_elements {
        let mut line = format!(
            "- id={} alias={} role={} name={} stable_key={}",
            element.id, element.alias, element.role, element.name, element.stable_key
        );
        if !element.security_flags.is_empty() {
            let safe_flags: Vec<String> = element
                .security_flags
                .iter()
                .map(|f| {
                    f.chars()
                        .filter(|c| c.is_ascii_alphanumeric() || *c == '_' || *c == '-')
                        .collect()
                })
                .filter(|f: &String| !f.is_empty())
                .collect();
            if !safe_flags.is_empty() {
                line.push_str(&format!(" security_flags={}", safe_flags.join(",")));
            }
        }
        lines.push(line);
    }

    lines.join("\n")
}

pub(crate) fn load_profile_name(profile: LoadProfile) -> &'static str {
    match profile {
        LoadProfile::Minimal => "minimal",
        LoadProfile::Visual => "visual",
        LoadProfile::Interactive => "interactive",
    }
}

pub(crate) fn approval_scope_name(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::ActionOnly => "action_only",
        ApprovalScope::UntilNavigation => "until_navigation",
        ApprovalScope::Timeboxed { .. } => "timeboxed",
    }
}

pub(crate) fn action_error_payload(error: &ActionError) -> Value {
    match error {
        ActionError::VerifyRequired => json!({ "status": "verify_required" }),
        ActionError::Blocked { rule_id } => {
            json!({ "status": "blocked", "rule_id": rule_id })
        }
        ActionError::HumanApprovalRequired {
            rule_id,
            scope,
            outcome,
        } => {
            let mut payload = json!({
                "status": "requires_human_approval",
                "rule_id": rule_id,
                "scope": approval_scope_name(*scope)
            });
            if let Some(projection) = outcome {
                match serde_json::to_value(projection) {
                    Ok(value) => payload["outcome_projection"] = value,
                    Err(error) => {
                        eprintln!("[mcp-server] failed to serialize outcome_projection: {error}");
                    }
                }
            }
            payload
        }
        ActionError::AskHumanRequired { reason } => json!({
            "status": "ask_human_required",
            "reason": reason
        }),
    }
}

pub(crate) fn skill_run_status_name(status: SkillRunStatus) -> &'static str {
    match status {
        SkillRunStatus::Completed => "completed",
        SkillRunStatus::Failed => "failed",
        SkillRunStatus::Handoff => "handoff",
    }
}

pub(crate) fn parse_attribute_value(raw: &str) -> Value {
    let trimmed = raw.trim();

    if trimmed.eq_ignore_ascii_case("true") {
        return Value::Bool(true);
    }
    if trimmed.eq_ignore_ascii_case("false") {
        return Value::Bool(false);
    }
    if let Ok(int_value) = trimmed.parse::<i64>() {
        return json!(int_value);
    }
    if let Ok(float_value) = trimmed.parse::<f64>() {
        return json!(float_value);
    }

    Value::String(raw.to_string())
}

pub(crate) fn infer_policy_flags(node: &SemanticNode) -> Vec<String> {
    let mut flags = Vec::new();
    let label = node.label.clone().unwrap_or_default().to_lowercase();
    if node.role == "button"
        && (label.contains("purchase") || label.contains("pay") || label.contains("checkout"))
    {
        flags.push("financial_transaction".to_string());
    }
    flags
}

/// Truncate a full SHA-256 hex key to the external short form.
/// If the input is already shorter than `STABLE_KEY_SHORT_LEN`, it is returned as-is.
pub(crate) fn shorten_key(key: &str) -> String {
    key.chars().take(STABLE_KEY_SHORT_LEN).collect()
}

pub(crate) fn fallback_stable_key(node: &SemanticNode) -> String {
    let mut hasher = Sha256::new();
    hasher.update(node.role.as_bytes());
    hasher.update(b":");
    hasher.update(node.label.clone().unwrap_or_default().as_bytes());
    hasher.update(b":");
    hasher.update(node.backend_node_id.to_string().as_bytes());
    shorten_key(&hex::encode(hasher.finalize()))
}

pub(crate) fn fallback_alias(node: &SemanticNode, stable_key: &str) -> String {
    let mut role = node.role.to_lowercase();
    role.retain(|ch| ch.is_ascii_alphanumeric() || ch == '_');
    if role.is_empty() {
        role = "element".to_string();
    }

    if node.backend_node_id > 0 {
        format!("{}_{}", role, node.backend_node_id)
    } else {
        let key_prefix: String = stable_key.chars().take(8).collect();
        format!("{}_{}", role, key_prefix)
    }
}
