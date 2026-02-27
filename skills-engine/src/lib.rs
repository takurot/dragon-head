use jsonschema::validator_for;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::collections::HashMap;
use thiserror::Error;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SkillDefinition {
    pub schema_version: u32,
    pub name: String,
    pub steps: Vec<SkillStep>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillStep {
    Locate(LocateStep),
    Verify(VerifyStep),
    Act(ActStep),
    Wait(WaitStep),
    Extract(ExtractStep),
    Handoff(HandoffStep),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct StepControl {
    #[serde(default)]
    pub max_retries: u32,
    #[serde(default)]
    pub on_success: Option<String>,
    #[serde(default)]
    pub on_failure: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LocateStep {
    #[serde(default)]
    pub id: Option<String>,
    pub query: String,
    #[serde(default)]
    pub control: StepControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct VerifyStep {
    #[serde(default)]
    pub id: Option<String>,
    pub target: String,
    pub expected: String,
    #[serde(default)]
    pub control: StepControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ActStep {
    #[serde(default)]
    pub id: Option<String>,
    pub action: String,
    pub target: String,
    #[serde(default)]
    pub value: Option<String>,
    #[serde(default)]
    pub control: StepControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct WaitStep {
    #[serde(default)]
    pub id: Option<String>,
    pub condition: String,
    pub timeout_ms: u64,
    #[serde(default)]
    pub control: StepControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct ExtractStep {
    #[serde(default)]
    pub id: Option<String>,
    pub key: String,
    pub selector: String,
    #[serde(default)]
    pub control: StepControl,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct HandoffStep {
    #[serde(default)]
    pub id: Option<String>,
    pub reason: String,
    #[serde(default)]
    pub assignee: Option<String>,
    #[serde(default)]
    pub control: StepControl,
}

impl SkillStep {
    fn id(&self) -> Option<&str> {
        match self {
            SkillStep::Locate(step) => step.id.as_deref(),
            SkillStep::Verify(step) => step.id.as_deref(),
            SkillStep::Act(step) => step.id.as_deref(),
            SkillStep::Wait(step) => step.id.as_deref(),
            SkillStep::Extract(step) => step.id.as_deref(),
            SkillStep::Handoff(step) => step.id.as_deref(),
        }
    }

    fn control(&self) -> &StepControl {
        match self {
            SkillStep::Locate(step) => &step.control,
            SkillStep::Verify(step) => &step.control,
            SkillStep::Act(step) => &step.control,
            SkillStep::Wait(step) => &step.control,
            SkillStep::Extract(step) => &step.control,
            SkillStep::Handoff(step) => &step.control,
        }
    }

    fn kind(&self) -> &'static str {
        match self {
            SkillStep::Locate(_) => "locate",
            SkillStep::Verify(_) => "verify",
            SkillStep::Act(_) => "act",
            SkillStep::Wait(_) => "wait",
            SkillStep::Extract(_) => "extract",
            SkillStep::Handoff(_) => "handoff",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OperationOutcome {
    Success,
    Failure { reason: String },
    Handoff { reason: String },
}

impl OperationOutcome {
    fn label(&self) -> &'static str {
        match self {
            OperationOutcome::Success => "success",
            OperationOutcome::Failure { .. } => "failure",
            OperationOutcome::Handoff { .. } => "handoff",
        }
    }
}

#[derive(Debug, Default, Clone)]
pub struct SkillExecutionContext {
    pub extracted: HashMap<String, Value>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OperationTrace {
    pub step_id: Option<String>,
    pub step_kind: String,
    pub operation: String,
    pub outcome: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SkillRunStatus {
    Completed,
    Failed,
    Handoff,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SkillRunReport {
    pub status: SkillRunStatus,
    pub trace: Vec<OperationTrace>,
    pub message: Option<String>,
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SkillEngineError {
    #[error("skill definition contains no steps")]
    EmptySkill,
    #[error("duplicate step id found: {step_id}")]
    DuplicateStepId { step_id: String },
    #[error("branch target '{target}' referenced by step '{step_id:?}' was not found")]
    UnknownBranchTarget {
        step_id: Option<String>,
        target: String,
    },
    #[error("act step '{step_id:?}' must be immediately preceded by verify step")]
    ActStepMissingVerifyPredecessor { step_id: Option<String> },
    #[error("skill execution exceeded operation limit {limit}")]
    OperationLimitExceeded { limit: usize },
    #[error("schema validation failed: {message}")]
    SchemaValidation { message: String },
    #[error("failed to deserialize skill json: {message}")]
    Deserialize { message: String },
}

#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum SkillSchemaError {
    #[error("failed to compile skill schema: {0}")]
    InvalidSchema(String),
    #[error("skill json failed schema validation: {0}")]
    ValidationErrors(String),
}

pub trait SkillRuntime {
    fn locate(&mut self, _step: &LocateStep, _ctx: &mut SkillExecutionContext) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn verify(&mut self, _step: &VerifyStep, _ctx: &mut SkillExecutionContext) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn policy_check(
        &mut self,
        _step: &ActStep,
        _ctx: &mut SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn act(&mut self, _step: &ActStep, _ctx: &mut SkillExecutionContext) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn post_check(
        &mut self,
        _step: &ActStep,
        _ctx: &mut SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn wait(&mut self, _step: &WaitStep, _ctx: &mut SkillExecutionContext) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn extract(
        &mut self,
        _step: &ExtractStep,
        _ctx: &mut SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Success
    }

    fn handoff(
        &mut self,
        _step: &HandoffStep,
        _ctx: &mut SkillExecutionContext,
    ) -> OperationOutcome {
        OperationOutcome::Handoff {
            reason: "handoff requested".to_string(),
        }
    }
}

pub struct SkillEngine {
    max_operations: usize,
}

impl Default for SkillEngine {
    fn default() -> Self {
        Self {
            max_operations: 1_024,
        }
    }
}

impl SkillEngine {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_max_operations(max_operations: usize) -> Self {
        Self { max_operations }
    }

    pub fn run<R: SkillRuntime>(
        &self,
        skill: &SkillDefinition,
        runtime: &mut R,
    ) -> Result<SkillRunReport, SkillEngineError> {
        validate_skill_definition(skill)?;
        let step_index = build_step_index(skill)?;

        let mut trace = Vec::new();
        let mut context = SkillExecutionContext::default();
        let mut retries_by_step: HashMap<usize, u32> = HashMap::new();

        let mut cursor = 0usize;
        let mut operations = 0usize;

        while cursor < skill.steps.len() {
            if operations >= self.max_operations {
                return Err(SkillEngineError::OperationLimitExceeded {
                    limit: self.max_operations,
                });
            }
            operations += 1;

            let step = &skill.steps[cursor];
            let outcome = self.execute_step(step, runtime, &mut context, &mut trace);

            match outcome {
                OperationOutcome::Success => {
                    retries_by_step.remove(&cursor);
                    if matches!(step, SkillStep::Handoff(_)) {
                        return Ok(SkillRunReport {
                            status: SkillRunStatus::Handoff,
                            trace,
                            message: Some("handoff step reached".to_string()),
                        });
                    }

                    if let Some(target) = step.control().on_success.as_deref() {
                        // Safe because branch targets are validated up front.
                        cursor = *step_index
                            .get(target)
                            .expect("validated branch target must exist");
                    } else {
                        cursor += 1;
                    }
                }
                OperationOutcome::Failure { reason } => {
                    let max_retries = step.control().max_retries;
                    let attempts = retries_by_step.entry(cursor).or_insert(0);
                    if *attempts < max_retries {
                        *attempts += 1;
                        continue;
                    }
                    retries_by_step.remove(&cursor);

                    if let Some(target) = step.control().on_failure.as_deref() {
                        cursor = *step_index
                            .get(target)
                            .expect("validated branch target must exist");
                        continue;
                    }

                    return Ok(SkillRunReport {
                        status: SkillRunStatus::Failed,
                        trace,
                        message: Some(reason),
                    });
                }
                OperationOutcome::Handoff { reason } => {
                    return Ok(SkillRunReport {
                        status: SkillRunStatus::Handoff,
                        trace,
                        message: Some(reason),
                    });
                }
            }
        }

        Ok(SkillRunReport {
            status: SkillRunStatus::Completed,
            trace,
            message: None,
        })
    }

    fn execute_step<R: SkillRuntime>(
        &self,
        step: &SkillStep,
        runtime: &mut R,
        context: &mut SkillExecutionContext,
        trace: &mut Vec<OperationTrace>,
    ) -> OperationOutcome {
        match step {
            SkillStep::Locate(def) => {
                call_operation(step, "locate", trace, || runtime.locate(def, context))
            }
            SkillStep::Verify(def) => {
                call_operation(step, "verify", trace, || runtime.verify(def, context))
            }
            SkillStep::Wait(def) => {
                call_operation(step, "wait", trace, || runtime.wait(def, context))
            }
            SkillStep::Extract(def) => {
                call_operation(step, "extract", trace, || runtime.extract(def, context))
            }
            SkillStep::Handoff(def) => {
                match call_operation(step, "handoff", trace, || runtime.handoff(def, context)) {
                    OperationOutcome::Success => OperationOutcome::Handoff {
                        reason: def.reason.clone(),
                    },
                    other => other,
                }
            }
            SkillStep::Act(def) => {
                let policy = call_operation(step, "policy_check", trace, || {
                    runtime.policy_check(def, context)
                });
                if !matches!(policy, OperationOutcome::Success) {
                    return policy;
                }

                let acted = call_operation(step, "act", trace, || runtime.act(def, context));
                if !matches!(acted, OperationOutcome::Success) {
                    return acted;
                }

                call_operation(step, "post_check", trace, || {
                    runtime.post_check(def, context)
                })
            }
        }
    }
}

fn call_operation<F>(
    step: &SkillStep,
    operation: &str,
    trace: &mut Vec<OperationTrace>,
    op: F,
) -> OperationOutcome
where
    F: FnOnce() -> OperationOutcome,
{
    let outcome = op();
    trace.push(OperationTrace {
        step_id: step.id().map(ToString::to_string),
        step_kind: step.kind().to_string(),
        operation: operation.to_string(),
        outcome: outcome.label().to_string(),
    });
    outcome
}

fn build_step_index(skill: &SkillDefinition) -> Result<HashMap<String, usize>, SkillEngineError> {
    let mut index = HashMap::new();
    for (idx, step) in skill.steps.iter().enumerate() {
        if let Some(step_id) = step.id()
            && index.insert(step_id.to_string(), idx).is_some()
        {
            return Err(SkillEngineError::DuplicateStepId {
                step_id: step_id.to_string(),
            });
        }
    }
    Ok(index)
}

pub fn validate_skill_definition(skill: &SkillDefinition) -> Result<(), SkillEngineError> {
    if skill.steps.is_empty() {
        return Err(SkillEngineError::EmptySkill);
    }

    let index = build_step_index(skill)?;

    for step in &skill.steps {
        let control = step.control();
        if let Some(target) = control.on_success.as_ref()
            && !index.contains_key(target)
        {
            return Err(SkillEngineError::UnknownBranchTarget {
                step_id: step.id().map(ToString::to_string),
                target: target.clone(),
            });
        }
        if let Some(target) = control.on_failure.as_ref()
            && !index.contains_key(target)
        {
            return Err(SkillEngineError::UnknownBranchTarget {
                step_id: step.id().map(ToString::to_string),
                target: target.clone(),
            });
        }
    }

    for idx in 0..skill.steps.len() {
        if let SkillStep::Act(act) = &skill.steps[idx]
            && (idx == 0 || !matches!(skill.steps[idx - 1], SkillStep::Verify(_)))
        {
            return Err(SkillEngineError::ActStepMissingVerifyPredecessor {
                step_id: act.id.clone(),
            });
        }
    }

    Ok(())
}

pub fn skill_definition_schema() -> Value {
    let control_schema = json!({
        "type": "object",
        "additionalProperties": false,
        "properties": {
            "max_retries": { "type": "integer", "minimum": 0 },
            "on_success": { "type": "string" },
            "on_failure": { "type": "string" }
        }
    });

    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "title": "SkillDefinition",
        "type": "object",
        "additionalProperties": false,
        "required": ["schema_version", "name", "steps"],
        "properties": {
            "schema_version": { "type": "integer", "minimum": 1 },
            "name": { "type": "string", "minLength": 1 },
            "steps": {
                "type": "array",
                "minItems": 1,
                "items": {
                    "oneOf": [
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "query"],
                            "properties": {
                                "type": { "const": "locate" },
                                "id": { "type": "string" },
                                "query": { "type": "string", "minLength": 1 },
                                "control": control_schema
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "target", "expected"],
                            "properties": {
                                "type": { "const": "verify" },
                                "id": { "type": "string" },
                                "target": { "type": "string", "minLength": 1 },
                                "expected": { "type": "string", "minLength": 1 },
                                "control": control_schema
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "action", "target"],
                            "properties": {
                                "type": { "const": "act" },
                                "id": { "type": "string" },
                                "action": { "type": "string", "minLength": 1 },
                                "target": { "type": "string", "minLength": 1 },
                                "value": { "type": ["string", "null"] },
                                "control": control_schema
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "condition", "timeout_ms"],
                            "properties": {
                                "type": { "const": "wait" },
                                "id": { "type": "string" },
                                "condition": { "type": "string", "minLength": 1 },
                                "timeout_ms": { "type": "integer", "minimum": 0 },
                                "control": control_schema
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "key", "selector"],
                            "properties": {
                                "type": { "const": "extract" },
                                "id": { "type": "string" },
                                "key": { "type": "string", "minLength": 1 },
                                "selector": { "type": "string", "minLength": 1 },
                                "control": control_schema
                            }
                        },
                        {
                            "type": "object",
                            "additionalProperties": false,
                            "required": ["type", "reason"],
                            "properties": {
                                "type": { "const": "handoff" },
                                "id": { "type": "string" },
                                "reason": { "type": "string", "minLength": 1 },
                                "assignee": { "type": "string" },
                                "control": control_schema
                            }
                        }
                    ]
                }
            }
        }
    })
}

pub fn validate_skill_json(value: &Value) -> Result<(), SkillSchemaError> {
    let schema = skill_definition_schema();
    let validator =
        validator_for(&schema).map_err(|err| SkillSchemaError::InvalidSchema(err.to_string()))?;

    let mut errors: Vec<String> = validator
        .iter_errors(value)
        .map(|error| error.to_string())
        .collect();

    if errors.is_empty() {
        return Ok(());
    }

    errors.sort();
    Err(SkillSchemaError::ValidationErrors(errors.join("; ")))
}

pub fn parse_skill_definition(value: &Value) -> Result<SkillDefinition, SkillEngineError> {
    validate_skill_json(value).map_err(|err| SkillEngineError::SchemaValidation {
        message: err.to_string(),
    })?;

    serde_json::from_value(value.clone()).map_err(|err| SkillEngineError::Deserialize {
        message: err.to_string(),
    })
}
