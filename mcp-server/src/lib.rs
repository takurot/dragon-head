use anyhow::{Context, Result};
use core_runtime::{
    sre::{LoadProfile, SemanticNode},
    ActionError, ApprovalScope, PageSession, VerifyError,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use skills_engine::{
    parse_skill_definition, ActStep, ExtractStep, LocateStep, OperationOutcome, SkillDefinition,
    SkillEngine, SkillRunStatus, SkillRuntime, VerifyStep, WaitStep,
};
use std::{
    collections::{BTreeMap, HashMap},
    time::Duration,
};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ToolDefinition {
    pub name: String,
    pub description: String,
    #[serde(rename = "inputSchema")]
    pub input_schema: Value,
}

pub trait McpBackend {
    fn get_state(&mut self, arguments: Value) -> Result<Value>;
    fn act(&mut self, arguments: Value) -> Result<Value>;
    fn verify(&mut self, arguments: Value) -> Result<Value>;
    fn get_visual(&mut self, arguments: Value) -> Result<Value>;
    fn ask_human(&mut self, arguments: Value) -> Result<Value>;
    fn run_skill(&mut self, arguments: Value) -> Result<Value>;
}

pub struct McpServer<B> {
    backend: B,
}

impl<B: McpBackend> McpServer<B> {
    pub fn new(backend: B) -> Self {
        Self { backend }
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "get_state".to_string(),
                description: "Retrieve the semantic page state".to_string(),
                input_schema: get_state_input_schema(),
            },
            ToolDefinition {
                name: "act".to_string(),
                description: "Execute an interaction action".to_string(),
                input_schema: act_input_schema(),
            },
            ToolDefinition {
                name: "verify".to_string(),
                description: "Verify precondition text before acting".to_string(),
                input_schema: verify_input_schema(),
            },
            ToolDefinition {
                name: "get_visual".to_string(),
                description: "Capture visual context with optional marks".to_string(),
                input_schema: get_visual_input_schema(),
            },
            ToolDefinition {
                name: "ask_human".to_string(),
                description: "Resolve pending HITL request".to_string(),
                input_schema: ask_human_input_schema(),
            },
            ToolDefinition {
                name: "run_skill".to_string(),
                description: "Execute a declarative skill workflow".to_string(),
                input_schema: run_skill_input_schema(),
            },
        ]
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        match name {
            "get_state" => self.backend.get_state(arguments),
            "act" => self.backend.act(arguments),
            "verify" => self.backend.verify(arguments),
            "get_visual" => self.backend.get_visual(arguments),
            "ask_human" => self.backend.ask_human(arguments),
            "run_skill" => self.backend.run_skill(arguments),
            _ => anyhow::bail!("unknown MCP tool: {name}"),
        }
    }

    pub fn handle_jsonrpc(&mut self, request: &str) -> String {
        let parsed = serde_json::from_str::<JsonRpcRequest>(request);
        let req = match parsed {
            Ok(req) => req,
            Err(err) => {
                return serialize_response(JsonRpcResponse::error(
                    Value::Null,
                    -32700,
                    format!("parse error: {err}"),
                ));
            }
        };

        let id = req.id.clone();
        let result: Result<Value, (i64, String)> = match req.method.as_str() {
            "tools/list" => Ok(json!({ "tools": self.tools() })),
            "tools/call" => {
                let name = req.params.get("name").and_then(Value::as_str);

                match name {
                    Some(name) => {
                        let arguments = req
                            .params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));

                        self.call_tool(name, arguments)
                            .map(|payload| {
                                json!({
                                    "content": [{
                                        "type": "json",
                                        "json": payload
                                    }]
                                })
                            })
                            .map_err(|err| (-32000, err.to_string()))
                    }
                    None => Err((
                        -32602,
                        "tools/call params.name must be a string".to_string(),
                    )),
                }
            }
            other => Err((-32601, format!("unsupported method: {other}"))),
        };

        match result {
            Ok(result) => serialize_response(JsonRpcResponse::success(id, result)),
            Err((code, message)) => serialize_response(JsonRpcResponse::error(id, code, message)),
        }
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcRequest {
    pub jsonrpc: String,
    pub id: Value,
    pub method: String,
    #[serde(default)]
    pub params: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcError {
    pub code: i64,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct JsonRpcResponse {
    pub jsonrpc: &'static str,
    pub id: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcResponse {
    fn success(id: Value, result: Value) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: Some(result),
            error: None,
        }
    }

    fn error(id: Value, code: i64, message: String) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            result: None,
            error: Some(JsonRpcError { code, message }),
        }
    }
}

fn serialize_response(response: JsonRpcResponse) -> String {
    serde_json::to_string(&response).unwrap_or_else(|err| {
        json!({
            "jsonrpc": "2.0",
            "id": Value::Null,
            "error": {
                "code": -32603,
                "message": format!("failed to serialize response: {err}")
            }
        })
        .to_string()
    })
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
enum StateFormat {
    #[default]
    Json,
    Markdown,
}

#[derive(Debug, Clone, Deserialize)]
struct GetStateArguments {
    #[serde(default)]
    format: StateFormat,
    #[serde(default)]
    force_refresh: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct ActArguments {
    #[serde(default)]
    target_id: Option<i64>,
    #[serde(default)]
    target_stable_key: Option<String>,
    action: String,
    #[serde(default)]
    value: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct VerifyArguments {
    target_id: i64,
    expected: VerifyExpected,
}

#[derive(Debug, Clone, Deserialize)]
struct VerifyExpected {
    text: String,
}

#[derive(Debug, Clone, Deserialize)]
struct GetVisualArguments {
    #[serde(default = "default_visual_mode")]
    mode: String,
    #[serde(default = "default_viewport")]
    viewport: String,
}

fn default_visual_mode() -> String {
    "som".to_string()
}

fn default_viewport() -> String {
    "full".to_string()
}

#[derive(Debug, Clone, Deserialize)]
struct AskHumanArguments {
    reason: String,
    #[serde(default)]
    context: bool,
}

#[derive(Debug, Clone, Deserialize)]
struct RunSkillArguments {
    skill_name: String,
    #[serde(default = "default_skill_params")]
    params: Value,
}

fn default_skill_params() -> Value {
    json!({})
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalSemanticState {
    pub metadata: StateMetadata,
    pub interactive_elements: Vec<ExternalInteractiveElement>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct StateMetadata {
    pub url: String,
    pub page_instance_id: String,
    pub state_hash: String,
    pub load_profile: String,
    pub timestamp: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ExternalInteractiveElement {
    pub id: i64,
    pub stable_key: String,
    pub alias: String,
    pub role: String,
    pub name: String,
    pub attributes: BTreeMap<String, Value>,
    pub bbox: [f64; 4],
    pub policy_flags: Vec<String>,
}

pub struct CoreRuntimeBackend {
    page: PageSession,
    state_cache: Option<ExternalSemanticState>,
    skill_engine: SkillEngine,
    skills: HashMap<String, SkillDefinition>,
}

impl CoreRuntimeBackend {
    pub fn new(page: PageSession) -> Self {
        Self {
            page,
            state_cache: None,
            skill_engine: SkillEngine::new(),
            skills: HashMap::new(),
        }
    }

    pub fn page(&self) -> &PageSession {
        &self.page
    }

    pub fn register_skill(&mut self, skill: SkillDefinition) {
        self.skills.insert(skill.name.clone(), skill);
    }

    pub fn register_skill_json(&mut self, value: &Value) -> Result<()> {
        let skill = parse_skill_definition(value)?;
        self.register_skill(skill);
        Ok(())
    }

    fn semantic_state_payload(&mut self, force_refresh: bool) -> Result<ExternalSemanticState> {
        if !force_refresh {
            if let Some(cached) = &self.state_cache {
                return Ok(cached.clone());
            }
        }

        let state = self.page.capture_semantic_state(LoadProfile::Interactive)?;
        let metadata = StateMetadata {
            url: self.page.current_url()?,
            page_instance_id: state.page_instance_id().to_string(),
            state_hash: state.state_hash().to_string(),
            load_profile: load_profile_name(state.load_profile()).to_string(),
            timestamp: state.timestamp(),
        };

        let interactive_elements = state
            .generate_fast_state()
            .interactive_elements
            .into_iter()
            .map(|node| self.map_interactive_element(node))
            .collect::<Result<Vec<_>>>()?;

        let payload = ExternalSemanticState {
            metadata,
            interactive_elements,
        };

        self.state_cache = Some(payload.clone());
        Ok(payload)
    }

    fn map_interactive_element(&self, node: SemanticNode) -> Result<ExternalInteractiveElement> {
        let id = node.backend_node_id;
        let stable_key = node
            .stable_key
            .clone()
            .filter(|key| !key.trim().is_empty())
            .unwrap_or_else(|| fallback_stable_key(&node));
        let alias = node
            .alias
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| fallback_alias(&node, &stable_key));
        let name = node
            .label
            .clone()
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| alias.clone());

        let bbox = if id > 0 {
            self.page
                .get_element_bbox(id)?
                .unwrap_or([0.0, 0.0, 0.0, 0.0])
        } else {
            [0.0, 0.0, 0.0, 0.0]
        };

        let policy_flags = infer_policy_flags(&node);
        let attributes = node
            .attributes
            .unwrap_or_default()
            .into_iter()
            .map(|(key, value)| (key, parse_attribute_value(&value)))
            .collect();

        Ok(ExternalInteractiveElement {
            id,
            stable_key,
            alias,
            role: node.role,
            name,
            attributes,
            bbox,
            policy_flags,
        })
    }
}

impl McpBackend for CoreRuntimeBackend {
    fn get_state(&mut self, arguments: Value) -> Result<Value> {
        let args: GetStateArguments = match serde_json::from_value(arguments) {
            Ok(value) => value,
            Err(_) => GetStateArguments {
                format: StateFormat::Json,
                force_refresh: false,
            },
        };

        let payload = self.semantic_state_payload(args.force_refresh)?;
        match args.format {
            StateFormat::Json => Ok(serde_json::to_value(payload)?),
            StateFormat::Markdown => Ok(json!({
                "markdown": render_state_markdown(&payload)
            })),
        }
    }

    fn act(&mut self, arguments: Value) -> Result<Value> {
        let args: ActArguments =
            serde_json::from_value(arguments).context("invalid act arguments")?;

        match self.page.act(
            args.target_id,
            args.target_stable_key.as_deref(),
            &args.action,
            args.value.as_deref(),
        ) {
            Ok(()) => {
                self.state_cache = None;
                Ok(json!({"status": "ok"}))
            }
            Err(err) => {
                if let Some(action_err) = err.downcast_ref::<ActionError>() {
                    let payload = match action_err {
                        ActionError::VerifyRequired => json!({ "status": "verify_required" }),
                        ActionError::Blocked { rule_id } => {
                            json!({ "status": "blocked", "rule_id": rule_id })
                        }
                        ActionError::HumanApprovalRequired { rule_id, scope } => json!({
                            "status": "requires_human_approval",
                            "rule_id": rule_id,
                            "scope": approval_scope_name(*scope)
                        }),
                    };
                    return Ok(payload);
                }
                Err(err.context("act tool failed"))
            }
        }
    }

    fn verify(&mut self, arguments: Value) -> Result<Value> {
        let args: VerifyArguments =
            serde_json::from_value(arguments).context("invalid verify arguments")?;

        match self.page.verify_text(args.target_id, &args.expected.text) {
            Ok(()) => Ok(json!({ "matched": true })),
            Err(err) => {
                if let Some(verify_err) = err.downcast_ref::<VerifyError>() {
                    return match verify_err {
                        VerifyError::ExpectationMismatch {
                            target_id,
                            expected,
                            actual,
                        } => Ok(json!({
                            "matched": false,
                            "target_id": target_id,
                            "expected": expected,
                            "actual": actual
                        })),
                    };
                }
                Err(err.context("verify tool failed"))
            }
        }
    }

    fn get_visual(&mut self, arguments: Value) -> Result<Value> {
        let args: GetVisualArguments =
            serde_json::from_value(arguments).context("invalid get_visual arguments")?;
        let capture = self.page.get_visual()?;

        let mut hasher = Sha256::new();
        hasher.update(&capture.image_png);
        let image_sha256 = hex::encode(hasher.finalize());

        let marks = if args.mode == "clean" {
            Vec::new()
        } else {
            capture
                .marks
                .into_iter()
                .map(|mark| {
                    json!({
                        "id": mark.id,
                        "stable_key": mark.stable_key,
                        "bbox": mark.bbox
                    })
                })
                .collect::<Vec<_>>()
        };

        Ok(json!({
            "mode": args.mode,
            "viewport": args.viewport,
            "image_sha256": image_sha256,
            "marks": marks
        }))
    }

    fn ask_human(&mut self, arguments: Value) -> Result<Value> {
        let args: AskHumanArguments =
            serde_json::from_value(arguments).context("invalid ask_human arguments")?;

        let Some(pending) = self.page.pending_policy_approval() else {
            return Ok(json!({
                "approved": false,
                "reason": args.reason,
                "pending": false
            }));
        };

        self.page.approve_pending_policy_action()?;

        let mut payload = json!({
            "approved": true,
            "reason": args.reason,
            "pending": false,
            "rule_id": pending.rule_id,
            "scope": approval_scope_name(pending.scope)
        });

        if args.context {
            payload["context"] = json!({
                "action": pending.action,
                "target_signature": pending.target_signature
            });
        }

        Ok(payload)
    }

    fn run_skill(&mut self, arguments: Value) -> Result<Value> {
        let args: RunSkillArguments =
            serde_json::from_value(arguments).context("invalid run_skill arguments")?;

        let Some(skill) = self.skills.get(&args.skill_name).cloned() else {
            return Ok(json!({
                "status": "not_found",
                "skill_name": args.skill_name
            }));
        };

        let mut runtime = PageSkillRuntime::new(&self.page, &args.params);
        let report = self
            .skill_engine
            .run(&skill, &mut runtime)
            .context("run_skill execution failed")?;

        Ok(json!({
            "status": skill_run_status_name(report.status),
            "message": report.message,
            "trace": report
                .trace
                .into_iter()
                .map(|entry| {
                    json!({
                        "step_id": entry.step_id,
                        "step_kind": entry.step_kind,
                        "operation": entry.operation,
                        "outcome": entry.outcome
                    })
                })
                .collect::<Vec<_>>()
        }))
    }
}

struct PageSkillRuntime<'a> {
    page: &'a PageSession,
    params: &'a Value,
}

impl<'a> PageSkillRuntime<'a> {
    fn new(page: &'a PageSession, params: &'a Value) -> Self {
        Self { page, params }
    }
}

impl SkillRuntime for PageSkillRuntime<'_> {
    fn locate(
        &mut self,
        _step: &LocateStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn verify(
        &mut self,
        step: &VerifyStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let target = resolve_param(&step.target, self.params);
        let expected = resolve_param(&step.expected, self.params);
        if let Some(id) = parse_target_id(&target) {
            return match self.page.verify_text(id, &expected) {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }

        OperationOutcome::Success
    }

    fn act(
        &mut self,
        step: &ActStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let target = resolve_param(&step.target, self.params);
        let action = resolve_param(&step.action, self.params);
        let value = step
            .value
            .as_deref()
            .map(|raw| resolve_param(raw, self.params));

        let target_id = parse_target_id(&target);
        let target_stable_key = parse_target_stable_key(&target);

        match self.page.act(
            target_id,
            target_stable_key.as_deref(),
            &action,
            value.as_deref(),
        ) {
            Ok(()) => OperationOutcome::Success,
            Err(err) => OperationOutcome::Failure {
                reason: err.to_string(),
            },
        }
    }

    fn wait(
        &mut self,
        step: &WaitStep,
        _ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let condition = resolve_param(&step.condition, self.params);
        if let Some(intent) = condition.strip_prefix("intent:") {
            return match self
                .page
                .wait_for_intent(intent.trim(), Duration::from_millis(step.timeout_ms))
            {
                Ok(()) => OperationOutcome::Success,
                Err(err) => OperationOutcome::Failure {
                    reason: err.to_string(),
                },
            };
        }

        OperationOutcome::Success
    }

    fn extract(
        &mut self,
        step: &ExtractStep,
        ctx: &mut skills_engine::SkillExecutionContext,
    ) -> OperationOutcome {
        let selector = resolve_param(&step.selector, self.params);
        let script = format!(
            "(() => {{ const el = document.querySelector({}); return el ? (el.innerText || el.textContent || '').trim() : null; }})()",
            serde_json::to_string(&selector).unwrap_or_else(|_| "\"\"".to_string())
        );

        match self.page.evaluate_script(&script) {
            Ok(object) => {
                ctx.extracted.insert(
                    step.key.clone(),
                    object.value.unwrap_or_else(|| Value::String(String::new())),
                );
                OperationOutcome::Success
            }
            Err(err) => OperationOutcome::Failure {
                reason: err.to_string(),
            },
        }
    }
}

fn resolve_param(template: &str, params: &Value) -> String {
    let trimmed = template.trim();
    if let Some(key) = trimmed
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
        .map(str::trim)
    {
        if let Some(value) = params.get(key) {
            if let Some(value) = value.as_str() {
                return value.to_string();
            }
            if value.is_number() || value.is_boolean() {
                return value.to_string();
            }
        }
    }

    template.to_string()
}

fn parse_target_id(target: &str) -> Option<i64> {
    target
        .trim()
        .strip_prefix("id:")
        .and_then(|raw| raw.trim().parse::<i64>().ok())
}

fn parse_target_stable_key(target: &str) -> Option<String> {
    target
        .trim()
        .strip_prefix("stable_key:")
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned)
}

fn render_state_markdown(payload: &ExternalSemanticState) -> String {
    let mut lines = vec![
        "# Semantic State".to_string(),
        format!("- URL: {}", payload.metadata.url),
        format!("- Page Instance ID: {}", payload.metadata.page_instance_id),
        format!("- State Hash: {}", payload.metadata.state_hash),
        format!("- Load Profile: {}", payload.metadata.load_profile),
        format!("- Timestamp: {}", payload.metadata.timestamp),
        String::new(),
        "## Interactive Elements".to_string(),
    ];

    for element in &payload.interactive_elements {
        lines.push(format!(
            "- id={} alias={} role={} name={} stable_key={}",
            element.id, element.alias, element.role, element.name, element.stable_key
        ));
    }

    lines.join("\n")
}

fn load_profile_name(profile: LoadProfile) -> &'static str {
    match profile {
        LoadProfile::Minimal => "minimal",
        LoadProfile::Visual => "visual",
        LoadProfile::Interactive => "interactive",
    }
}

fn approval_scope_name(scope: ApprovalScope) -> &'static str {
    match scope {
        ApprovalScope::ActionOnly => "action_only",
        ApprovalScope::UntilNavigation => "until_navigation",
        ApprovalScope::Timeboxed { .. } => "timeboxed",
    }
}

fn skill_run_status_name(status: SkillRunStatus) -> &'static str {
    match status {
        SkillRunStatus::Completed => "completed",
        SkillRunStatus::Failed => "failed",
        SkillRunStatus::Handoff => "handoff",
    }
}

fn parse_attribute_value(raw: &str) -> Value {
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

fn infer_policy_flags(node: &SemanticNode) -> Vec<String> {
    let mut flags = Vec::new();
    let label = node.label.clone().unwrap_or_default().to_lowercase();
    if node.role == "button"
        && (label.contains("purchase") || label.contains("pay") || label.contains("checkout"))
    {
        flags.push("financial_transaction".to_string());
    }
    flags
}

fn fallback_stable_key(node: &SemanticNode) -> String {
    let mut hasher = Sha256::new();
    hasher.update(node.role.as_bytes());
    hasher.update(b":");
    hasher.update(node.label.clone().unwrap_or_default().as_bytes());
    hasher.update(b":");
    hasher.update(node.backend_node_id.to_string().as_bytes());
    hex::encode(hasher.finalize())
}

fn fallback_alias(node: &SemanticNode, stable_key: &str) -> String {
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

pub fn semantic_state_json_schema() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SemanticState",
        "type": "object",
        "additionalProperties": false,
        "required": ["metadata", "interactive_elements"],
        "properties": {
            "metadata": {
                "type": "object",
                "additionalProperties": false,
                "required": ["url", "page_instance_id", "state_hash", "load_profile", "timestamp"],
                "properties": {
                    "url": { "type": "string", "minLength": 1 },
                    "page_instance_id": { "type": "string", "minLength": 1 },
                    "state_hash": { "type": "string", "minLength": 1 },
                    "load_profile": { "type": "string", "enum": ["minimal", "visual", "interactive"] },
                    "timestamp": { "type": "integer", "minimum": 0 }
                }
            },
            "interactive_elements": {
                "type": "array",
                "items": {
                    "type": "object",
                    "additionalProperties": false,
                    "required": ["id", "stable_key", "alias", "role", "name", "attributes", "bbox", "policy_flags"],
                    "properties": {
                        "id": { "type": "integer" },
                        "stable_key": { "type": "string", "minLength": 1 },
                        "alias": { "type": "string", "minLength": 1 },
                        "role": { "type": "string", "minLength": 1 },
                        "name": { "type": "string" },
                        "attributes": {
                            "type": "object",
                            "additionalProperties": {
                                "oneOf": [
                                    { "type": "string" },
                                    { "type": "number" },
                                    { "type": "integer" },
                                    { "type": "boolean" },
                                    { "type": "null" }
                                ]
                            }
                        },
                        "bbox": {
                            "type": "array",
                            "items": { "type": "number" },
                            "minItems": 4,
                            "maxItems": 4
                        },
                        "policy_flags": {
                            "type": "array",
                            "items": { "type": "string" }
                        }
                    }
                }
            }
        }
    })
}

fn get_state_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "format": {
                "type": "string",
                "enum": ["json", "markdown"]
            },
            "force_refresh": {
                "type": "boolean"
            }
        }
    })
}

fn act_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["action"],
        "properties": {
            "target_id": { "type": "integer" },
            "target_stable_key": { "type": "string" },
            "action": {
                "type": "string",
                "enum": ["click", "type"]
            },
            "value": { "type": "string" }
        }
    })
}

fn verify_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["target_id", "expected"],
        "properties": {
            "target_id": { "type": "integer" },
            "expected": {
                "type": "object",
                "additionalProperties": false,
                "required": ["text"],
                "properties": {
                    "text": { "type": "string", "minLength": 1 }
                }
            }
        }
    })
}

fn get_visual_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "mode": {
                "type": "string",
                "enum": ["clean", "som"]
            },
            "viewport": {
                "type": "string",
                "enum": ["full"]
            }
        }
    })
}

fn ask_human_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["reason"],
        "properties": {
            "reason": { "type": "string", "minLength": 1 },
            "context": { "type": "boolean" }
        }
    })
}

fn run_skill_input_schema() -> Value {
    json!({
        "type": "object",
        "additionalProperties": false,
        "required": ["skill_name"],
        "properties": {
            "skill_name": { "type": "string", "minLength": 1 },
            "params": { "type": "object" }
        }
    })
}
