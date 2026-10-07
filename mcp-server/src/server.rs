use super::*;

#[derive(Debug, thiserror::Error)]
#[error("invalid arguments for tool `{tool}`")]
pub(crate) struct InvalidToolArguments {
    pub(crate) tool: String,
}

pub(crate) fn is_known_tool(name: &str) -> bool {
    matches!(
        name,
        "navigate"
            | "get_state"
            | "act"
            | "verify"
            | "get_visual"
            | "ask_human"
            | "run_skill"
            | "get_usage_report"
            | "extract"
    )
}

/// Tools whose CDP call is safe to retry once on a transport-level error
/// (ISSUE-260) because they are read-only / idempotent: re-issuing them
/// cannot double-fire a state-changing action. `navigate`, `act`,
/// `run_skill`, and `ask_human` are deliberately excluded -- a lost response
/// after Chrome already executed the command must not be retried blindly.
pub(crate) fn is_retry_safe_tool(name: &str) -> bool {
    matches!(name, "get_state" | "verify" | "get_visual" | "extract")
}

pub(crate) fn validate_tool_arguments(name: &str, arguments: &Value) -> Result<()> {
    static VALIDATORS: OnceLock<HashMap<&'static str, jsonschema::Validator>> = OnceLock::new();
    let validators = VALIDATORS.get_or_init(|| {
        [
            ("navigate", navigate_input_schema()),
            ("get_state", get_state_input_schema()),
            ("act", act_input_schema()),
            ("verify", verify_input_schema()),
            ("get_visual", get_visual_input_schema()),
            ("ask_human", ask_human_input_schema()),
            ("run_skill", run_skill_input_schema()),
            ("get_usage_report", get_usage_report_input_schema()),
            ("extract", extract_input_schema()),
        ]
        .into_iter()
        .map(|(tool, schema)| {
            let validator = jsonschema::validator_for(&schema)
                .unwrap_or_else(|error| panic!("invalid input schema for {tool}: {error}"));
            (tool, validator)
        })
        .collect()
    });

    let Some(validator) = validators.get(name) else {
        anyhow::bail!("unknown MCP tool: {name}");
    };
    if validator.is_valid(arguments) {
        Ok(())
    } else {
        Err(InvalidToolArguments {
            tool: name.to_string(),
        }
        .into())
    }
}

pub(crate) struct ToolCallOutput {
    pub(crate) structured_content: Value,
    pub(crate) visual_image: Option<Vec<u8>>,
}

pub(crate) fn validate_visual_image(image: &[u8], max_bytes: usize) -> Result<()> {
    if image.len() > max_bytes {
        anyhow::bail!(
            "get_visual image exceeds maximum size of {max_bytes} bytes (received {} bytes)",
            image.len()
        );
    }
    if !image.starts_with(PNG_SIGNATURE) {
        anyhow::bail!("get_visual capture is not a valid PNG image");
    }
    Ok(())
}

pub(crate) fn image_content_block(image: &[u8], structured_content: &Value) -> Result<Value> {
    validate_visual_image(image, MAX_VISUAL_IMAGE_BYTES)?;
    let reported_hash = structured_content
        .get("image_sha256")
        .and_then(Value::as_str)
        .context("get_visual metadata is missing image_sha256")?;
    let actual_hash = hex::encode(Sha256::digest(image));
    if reported_hash != actual_hash {
        anyhow::bail!("get_visual image_sha256 does not match captured image bytes");
    }
    Ok(json!({
        "type": "image",
        "data": BASE64_STANDARD.encode(image),
        "mimeType": "image/png"
    }))
}

impl<B: McpBackend> McpServer<B> {
    pub fn new(backend: B) -> Self {
        Self::new_with_plan(backend, PlanTier::Enterprise)
    }

    pub fn new_with_plan(backend: B, plan_tier: PlanTier) -> Self {
        Self {
            backend,
            plan_tier,
            usage_meters: UsageMeters::default(),
            consecutive_alive_skips: 0,
        }
    }

    pub fn tools(&self) -> Vec<ToolDefinition> {
        vec![
            ToolDefinition {
                name: "navigate".to_string(),
                description: "Navigate the current page to an HTTP(S) URL".to_string(),
                input_schema: navigate_input_schema(),
            },
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
            ToolDefinition {
                name: "get_usage_report".to_string(),
                description: "Retrieve usage meters and plan tier summary".to_string(),
                input_schema: get_usage_report_input_schema(),
            },
            ToolDefinition {
                name: "extract".to_string(),
                description: "Extract structured data using Deep Lens DSL rule".to_string(),
                input_schema: extract_input_schema(),
            },
        ]
    }

    pub fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value> {
        Ok(self.call_tool_output(name, arguments)?.structured_content)
    }

    /// Dispatches a single tool call to the backend. Extracted from
    /// [`call_tool_output`](Self::call_tool_output) so the ISSUE-260
    /// transport-error retry can invoke the same call twice.
    pub(crate) fn dispatch_tool(&mut self, name: &str, arguments: &Value) -> Result<Value> {
        match name {
            "navigate" => self.backend.navigate(arguments.clone()),
            "get_state" => self.backend.get_state(arguments.clone()),
            "act" => self.backend.act(arguments.clone()),
            "verify" => self.backend.verify(arguments.clone()),
            "get_visual" => self.backend.get_visual(arguments.clone()),
            "ask_human" => self.backend.ask_human(arguments.clone()),
            "run_skill" => self.backend.run_skill(arguments.clone()),
            "extract" => self.backend.extract(arguments.clone()),
            _ => anyhow::bail!("unknown MCP tool: {name}"),
        }
    }

    pub(crate) fn call_tool_output(
        &mut self,
        name: &str,
        arguments: Value,
    ) -> Result<ToolCallOutput> {
        if !is_known_tool(name) {
            anyhow::bail!("unknown MCP tool: {name}");
        }
        validate_tool_arguments(name, &arguments)?;

        if name == "get_usage_report" {
            return Ok(ToolCallOutput {
                structured_content: self.get_usage_report_payload()?,
                visual_image: None,
            });
        }

        if let Some(payload) = self.check_plan_gate(name, &arguments) {
            return Ok(ToolCallOutput {
                structured_content: payload,
                visual_image: None,
            });
        }

        let speculative_hits_before = self.backend.speculative_usage().0;

        let mut result = self.dispatch_tool(name, &arguments);

        // ISSUE-260: transport-level errors (broken pipe / connection reset)
        // can be a transient CDP websocket blip rather than an actual Chrome
        // crash. Retry the call once before treating it as a disconnect --
        // but only for read-only/idempotent tools. Retrying `act`,
        // `navigate`, `run_skill`, or `ask_human` blindly risks double-firing
        // a click/submit/navigation if the original CDP command actually
        // reached Chrome and only the *response* was lost in transit.
        if is_retry_safe_tool(name) {
            if let Err(err) = &result {
                if core_runtime::is_transport_error(err) {
                    result = self.dispatch_tool(name, &arguments);
                }
            }
        }

        let result = match result {
            Err(err) if core_runtime::is_browser_disconnected(&err) => {
                // ISSUE-260: a disconnect-shaped error doesn't necessarily
                // mean the Chrome process died -- probe process liveness
                // when the backend supports it, and skip the (destructive)
                // full session restart if Chrome is confirmed still alive.
                // Bounded by MAX_ALIVE_SKIPS_BEFORE_RESTART so a session
                // whose CDP transport is genuinely and permanently broken
                // (e.g. a stale PID reused by an unrelated process, or a tab
                // that never recovers) still eventually restarts rather than
                // silently failing forever.
                if self.backend.is_chrome_process_alive() == Some(true)
                    && self.consecutive_alive_skips < MAX_ALIVE_SKIPS_BEFORE_RESTART
                {
                    self.consecutive_alive_skips += 1;
                    Err(err)
                } else if self.backend.confirm_browser_disconnected() {
                    // ISSUE-261: a marker match alone can be a false positive
                    // when a slow-but-alive operation (e.g. `get_visual`'s
                    // screenshot + SoM pipeline) causes a client-side request
                    // timeout that a later command then observes as a
                    // transport-level error. Confirm true unresponsiveness
                    // with a bounded CDP health-check before committing to a
                    // restart; if the browser answers, propagate the original
                    // error instead so it isn't misdiagnosed as a crash.
                    self.consecutive_alive_skips = 0;
                    match self.backend.handle_browser_disconnect() {
                        Ok(restart_count) => {
                            Err(SessionError::BrowserRestarted { restart_count }.into())
                        }
                        Err(reason) => Err(SessionError::BrowserRestartFailed { reason }.into()),
                    }
                } else {
                    self.consecutive_alive_skips = 0;
                    Err(err)
                }
            }
            other => {
                if other.is_ok() {
                    self.consecutive_alive_skips = 0;
                }
                other
            }
        };

        let payload = result?;
        if !payload.is_object() {
            anyhow::bail!("MCP structuredContent must be a JSON object");
        }
        let visual_image = if name == "get_visual" {
            self.backend.take_visual_image()
        } else {
            None
        };
        let speculative_hit = self.backend.speculative_usage().0 > speculative_hits_before;
        self.record_usage(name, &arguments, &payload, speculative_hit);

        Ok(ToolCallOutput {
            structured_content: payload,
            visual_image,
        })
    }

    pub fn handle_jsonrpc(&mut self, request: &str) -> Option<String> {
        let parsed = serde_json::from_str::<JsonRpcRequest>(request);
        let req = match parsed {
            Ok(req) => req,
            Err(err) => {
                return Some(serialize_response(JsonRpcResponse::error(
                    Value::Null,
                    -32700,
                    format!("parse error: {err}"),
                )));
            }
        };

        let is_notification = req.id == Value::Null;

        let result: Result<Value, (i64, String)> = match req.method.as_str() {
            "initialize" => {
                let requested_version = req.params.get("protocolVersion").and_then(Value::as_str);
                let negotiated_version = negotiate_protocol_version(requested_version);

                if let Some(client_info) = req.params.get("clientInfo") {
                    let name = client_info
                        .get("name")
                        .and_then(Value::as_str)
                        .map(sanitize_log_field)
                        .unwrap_or_else(|| "unknown".to_string());
                    let version = client_info
                        .get("version")
                        .and_then(Value::as_str)
                        .map(sanitize_log_field)
                        .unwrap_or_else(|| "unknown".to_string());
                    eprintln!(
                        "[mcp-server] client connected: name={name} version={version} \
                         requested_protocol={}",
                        requested_version
                            .map(sanitize_log_field)
                            .unwrap_or_else(|| "none".to_string())
                    );
                }

                Ok(json!({
                    "protocolVersion": negotiated_version,
                    "capabilities": {
                        "tools": {}
                    },
                    "serverInfo": {
                        "name": "dragon-head-mcp",
                        "version": env!("CARGO_PKG_VERSION")
                    }
                }))
            }
            "notifications/initialized" => {
                return None;
            }
            "tools/list" => Ok(json!({ "tools": self.tools() })),
            "tools/call" => {
                let name = req.params.get("name").and_then(Value::as_str);

                match name {
                    Some(name) if is_known_tool(name) => {
                        let arguments = req
                            .params
                            .get("arguments")
                            .cloned()
                            .unwrap_or_else(|| json!({}));

                        self.call_tool_output(name, arguments)
                            .and_then(|output| {
                                let text = serde_json::to_string(&output.structured_content)
                                    .expect("serializing a serde_json::Value cannot fail");
                                let mut content = vec![json!({
                                    "type": "text",
                                    "text": text
                                })];
                                if let Some(image) = output.visual_image {
                                    content.push(image_content_block(
                                        &image,
                                        &output.structured_content,
                                    )?);
                                }
                                Ok(json!({
                                    "content": content,
                                    "structuredContent": output.structured_content
                                }))
                            })
                            .map_err(|err| {
                                if err.downcast_ref::<InvalidToolArguments>().is_some() {
                                    (-32602, err.to_string())
                                } else {
                                    (-32000, err.to_string())
                                }
                            })
                    }
                    Some(unknown) => Err((-32601, format!("unknown tool: {unknown}"))),
                    None => Err((
                        -32602,
                        "tools/call params.name must be a string".to_string(),
                    )),
                }
            }
            other => Err((-32601, format!("unsupported method: {other}"))),
        };

        if is_notification {
            return None;
        }

        let id = req.id;
        Some(match result {
            Ok(result) => serialize_response(JsonRpcResponse::success(id, result)),
            Err((code, message)) => serialize_response(JsonRpcResponse::error(id, code, message)),
        })
    }

    pub fn backend_mut(&mut self) -> &mut B {
        &mut self.backend
    }

    pub(crate) fn get_usage_report_payload(&self) -> Result<Value> {
        let (_, speculative_misses) = self.backend.speculative_usage();
        let report = self.usage_meters.to_report(
            self.plan_tier,
            self.backend.audit_retention_snapshot().unwrap_or_default(),
            self.backend.browser_restart_count(),
            speculative_misses,
        );
        serde_json::to_value(report).context("failed to serialize usage report")
    }

    pub(crate) fn check_plan_gate(&self, name: &str, arguments: &Value) -> Option<Value> {
        match name {
            "get_state" => {
                let args = parse_get_state_arguments(arguments)
                    .expect("tool arguments were validated before plan gating");
                if args.delivery == StateDelivery::Delta {
                    return self
                        .ensure_plan_feature(PlanFeature::SemanticDelta)
                        .map(|required| {
                            self.plan_upgrade_required_payload(PlanFeature::SemanticDelta, required)
                        });
                }
                None
            }
            "get_visual" => {
                let mode = arguments
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("som");
                if !mode.eq_ignore_ascii_case("clean") {
                    return self.ensure_plan_feature(PlanFeature::SomVisualCapture).map(
                        |required| {
                            self.plan_upgrade_required_payload(
                                PlanFeature::SomVisualCapture,
                                required,
                            )
                        },
                    );
                }
                None
            }
            "ask_human" => self
                .ensure_plan_feature(PlanFeature::PolicyHumanApproval)
                .map(|required| {
                    self.plan_upgrade_required_payload(PlanFeature::PolicyHumanApproval, required)
                }),
            _ => None,
        }
    }

    pub(crate) fn ensure_plan_feature(&self, feature: PlanFeature) -> Option<PlanTier> {
        let required_plan = feature.required_plan();
        if self.plan_tier >= required_plan {
            None
        } else {
            Some(required_plan)
        }
    }

    pub(crate) fn plan_upgrade_required_payload(
        &self,
        feature: PlanFeature,
        required_plan: PlanTier,
    ) -> Value {
        json!({
            "status": "plan_upgrade_required",
            "feature": feature.as_str(),
            "required_plan": required_plan.as_str(),
            "current_plan": self.plan_tier.as_str()
        })
    }

    pub(crate) fn record_usage(
        &mut self,
        name: &str,
        arguments: &Value,
        payload: &Value,
        speculative_hit: bool,
    ) {
        match name {
            "get_state" => {
                let args = parse_get_state_arguments(arguments)
                    .expect("tool arguments were validated before usage metering");
                match args.delivery {
                    StateDelivery::Delta => {
                        self.usage_meters.state_generations.delta += 1;
                    }
                    StateDelivery::Full if speculative_hit => {
                        self.usage_meters.state_generations.speculative += 1;
                    }
                    StateDelivery::Full => {
                        self.usage_meters.state_generations.fast += 1;
                        self.usage_meters.state_generations.full += 1;
                    }
                }
            }
            "get_visual" => {
                let mode = arguments
                    .get("mode")
                    .and_then(Value::as_str)
                    .unwrap_or("som");
                if !mode.eq_ignore_ascii_case("clean")
                    && payload
                        .get("image_sha256")
                        .and_then(Value::as_str)
                        .is_some()
                {
                    self.usage_meters.visual_captures += 1;
                }
            }
            "act" | "navigate" => match payload.get("status").and_then(Value::as_str) {
                Some("ok") => {
                    self.usage_meters.actions_executed += 1;
                }
                Some("requires_human_approval") => {
                    self.usage_meters.hitl_events += 1;
                }
                _ => {}
            },
            "ask_human"
                if payload
                    .get("approved")
                    .and_then(Value::as_bool)
                    .unwrap_or(false) =>
            {
                self.usage_meters.hitl_events += 1;
            }
            "run_skill" => {
                let delta = self.backend.take_skill_usage_delta();
                self.usage_meters.actions_executed += delta.actions_executed;
                self.usage_meters.visual_captures += delta.visual_captures;
                self.usage_meters.hitl_events += delta.hitl_events;
            }
            _ => {}
        }
    }
}
