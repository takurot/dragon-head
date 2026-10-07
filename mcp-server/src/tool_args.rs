use super::*;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NavigateArguments {
    pub(crate) url: String,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateFormat {
    #[default]
    Json,
    Markdown,
}

#[derive(Debug, Clone, Copy, Deserialize, Serialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub(crate) enum StateDelivery {
    #[default]
    Full,
    Delta,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetStateArguments {
    #[serde(default)]
    pub(crate) format: StateFormat,
    #[serde(default)]
    pub(crate) force_refresh: bool,
    #[serde(default)]
    pub(crate) delivery: StateDelivery,
}

impl Default for GetStateArguments {
    fn default() -> Self {
        Self {
            format: StateFormat::Json,
            force_refresh: false,
            delivery: StateDelivery::Full,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct ActArguments {
    #[serde(default)]
    pub(crate) target_id: Option<i64>,
    #[serde(default)]
    pub(crate) target_stable_key: Option<String>,
    pub(crate) action: String,
    #[serde(default)]
    pub(crate) value: Option<String>,
}

/// Builds a [`ActionSignature`] identifying an `act` call for the speculative
/// state generation pipeline (Spec §3.5 / ISSUE-147). The signature combines
/// the action kind, target, and (for `type`) a digest of the value so that
/// distinct inputs to the same element are tracked as distinct transitions.
///
/// `value` is hashed rather than copied verbatim: for `type` actions it may
/// contain passwords, tokens, or other personal data, and `ActionSignature`s
/// are retained in the speculative engine's transition model beyond the
/// request, bypassing the audit log's argument redaction.
pub(crate) fn action_signature_for_act(args: &ActArguments) -> ActionSignature {
    let target = args
        .target_stable_key
        .clone()
        .or_else(|| args.target_id.map(|id| id.to_string()))
        .unwrap_or_default();
    let mut signature = format!("{}:{}", args.action, target);
    if let Some(value) = &args.value {
        let mut hasher = Sha256::new();
        hasher.update(value.as_bytes());
        signature.push(':');
        signature.push_str(&hex::encode(hasher.finalize()));
    }
    ActionSignature::from(signature)
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyArguments {
    pub(crate) target_id: i64,
    #[serde(default)]
    pub(crate) target_stable_key: Option<String>,
    pub(crate) expected: VerifyExpected,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct VerifyExpected {
    pub(crate) text: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct GetVisualArguments {
    #[serde(default = "default_visual_mode")]
    pub(crate) mode: String,
    #[serde(default = "default_viewport")]
    pub(crate) viewport: String,
}

pub(crate) fn default_visual_mode() -> String {
    "som".to_string()
}

pub(crate) fn default_viewport() -> String {
    "full".to_string()
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AskHumanArguments {
    pub(crate) reason: String,
    #[serde(default)]
    pub(crate) context: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct RunSkillArguments {
    pub(crate) skill_name: String,
    #[serde(default = "default_skill_params")]
    pub(crate) params: Value,
}

pub(crate) fn default_skill_params() -> Value {
    json!({})
}

pub(crate) fn parse_get_state_arguments(arguments: &Value) -> Result<GetStateArguments> {
    serde_json::from_value(arguments.clone()).context("invalid get_state arguments")
}
