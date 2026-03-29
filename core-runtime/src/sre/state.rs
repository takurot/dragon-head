use super::profile::LoadProfile;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// Semantic State: the deterministic, AI-optimized representation of a web page.
/// SPEC SRE-01: output includes page_instance_id, state_hash, timestamp, load_profile.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticState {
    page_instance_id: String,
    state_hash: String,
    timestamp: u64,
    load_profile: LoadProfile,
    root: SemanticNode,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct SemanticNode {
    pub role: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub label: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<SemanticNode>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<BTreeMap<String, String>>,

    /// Stable key for element identity (SHA-256 hex)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stable_key: Option<String>,

    /// True if the stable key collided and was resolved by index
    #[serde(default)]
    pub ambiguous: bool,

    /// Human-readable alias (if available)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub alias: Option<String>,

    // New field for ACT-04
    #[serde(default, rename = "id")]
    pub backend_node_id: i64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StateGenerationPhase {
    Fast,
    Full,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FastSemanticState {
    pub interactive_elements: Vec<SemanticNode>,
    pub messages: Vec<SemanticNode>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct FullSemanticState {
    pub forms: Vec<SemanticNode>,
    pub regions: Vec<SemanticNode>,
}

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct LayeredSemanticState {
    pub fast: FastSemanticState,
    pub full: FullSemanticState,
    pub generation_trace: Vec<StateGenerationPhase>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct DeltaPolicy {
    /// Maximum delta size relative to full-state payload.
    /// If patch_bytes / full_bytes exceeds this ratio, send full state.
    pub max_patch_bytes_ratio: f32,
    /// Maximum number of RFC 6902 operations allowed in one delta.
    pub max_operations: usize,
}

impl Default for DeltaPolicy {
    fn default() -> Self {
        Self {
            max_patch_bytes_ratio: 0.60,
            max_operations: 64,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SemanticDelta {
    pub previous_state_hash: String,
    pub next_state_hash: String,
    pub patch: json_patch::Patch,
}

impl SemanticDelta {
    pub fn operation_count(&self) -> usize {
        self.patch.0.len()
    }

    pub fn patch_size_bytes(&self) -> usize {
        serde_json::to_vec(&self.patch)
            .map(|bytes| bytes.len())
            .unwrap_or_default()
    }

    pub fn apply_to_root(&self, base_root: &SemanticNode) -> Result<SemanticNode> {
        let mut json_root =
            serde_json::to_value(base_root).context("Failed to encode base root")?;
        json_patch::patch(&mut json_root, &self.patch).context("Failed to apply RFC 6902 patch")?;
        serde_json::from_value(json_root).context("Failed to decode patched semantic root")
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum StateUpdate {
    Noop { state_hash: String },
    Full { state: SemanticState },
    Delta { delta: SemanticDelta },
}

impl SemanticState {
    /// Create a new SemanticState. The state_hash is computed from the root content only,
    /// ensuring that identical semantic content always produces the same hash.
    pub fn new(root: SemanticNode, load_profile: LoadProfile) -> Self {
        let state_hash = Self::compute_hash(&root);
        Self {
            page_instance_id: uuid::Uuid::new_v4().to_string(),
            state_hash,
            timestamp: std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs(),
            load_profile,
            root,
        }
    }

    /// Accessor for state_hash (read-only).
    pub fn state_hash(&self) -> &str {
        &self.state_hash
    }

    /// Accessor for root (read-only).
    pub fn root(&self) -> &SemanticNode {
        &self.root
    }

    /// Accessor for load_profile (read-only).
    pub fn load_profile(&self) -> LoadProfile {
        self.load_profile
    }

    /// Accessor for page_instance_id (read-only).
    pub fn page_instance_id(&self) -> &str {
        &self.page_instance_id
    }

    /// Accessor for timestamp (read-only, epoch seconds).
    pub fn timestamp(&self) -> u64 {
        self.timestamp
    }

    /// Build Fast State (`interactive_elements`, `messages`) only.
    pub fn generate_fast_state(&self) -> FastSemanticState {
        FastSemanticState {
            interactive_elements: collect_nodes_matching(&self.root, is_interactive_node),
            messages: collect_nodes_matching(&self.root, is_message_node),
        }
    }

    /// Build Full State (`forms`, `regions`) only.
    pub fn generate_full_state(&self) -> FullSemanticState {
        FullSemanticState {
            forms: collect_nodes_matching(&self.root, is_form_node),
            regions: collect_nodes_matching(&self.root, is_region_node),
        }
    }

    /// Build SRE-01 layered output.
    /// Fast State (`interactive_elements`, `messages`) is always generated first,
    /// then Full State (`forms`, `regions`) is generated.
    pub fn generate_layered_state(&self) -> LayeredSemanticState {
        let mut generation_trace = Vec::with_capacity(2);
        let fast = self.generate_fast_state_with_trace(&mut generation_trace);
        let full = self.generate_full_state_with_trace(&fast, &mut generation_trace);

        LayeredSemanticState {
            fast,
            full,
            generation_trace,
        }
    }

    /// Build an RFC 6902 semantic delta from a previous state.
    /// Returns `Ok(None)` when there is no content change.
    pub fn build_delta(&self, previous: &SemanticState) -> Result<Option<SemanticDelta>> {
        if self.state_hash() == previous.state_hash() {
            return Ok(None);
        }

        let previous_root_json =
            serde_json::to_value(previous.root()).context("Failed to encode previous root")?;
        let next_root_json =
            serde_json::to_value(self.root()).context("Failed to encode next root")?;
        let patch = json_patch::diff(&previous_root_json, &next_root_json);

        if patch.0.is_empty() {
            return Ok(None);
        }

        Ok(Some(SemanticDelta {
            previous_state_hash: previous.state_hash().to_string(),
            next_state_hash: self.state_hash().to_string(),
            patch,
        }))
    }

    /// Select delivery mode (no-op, full state, or semantic delta) based on policy.
    pub fn select_update(
        &self,
        previous: Option<&SemanticState>,
        policy: DeltaPolicy,
    ) -> Result<StateUpdate> {
        let Some(previous_state) = previous else {
            return Ok(StateUpdate::Full {
                state: self.clone(),
            });
        };

        let Some(delta) = self.build_delta(previous_state)? else {
            return Ok(StateUpdate::Noop {
                state_hash: self.state_hash().to_string(),
            });
        };

        let full_payload_bytes = full_update_payload_size_bytes(self)?;
        if should_send_delta(full_payload_bytes, &delta, policy) {
            Ok(StateUpdate::Delta { delta })
        } else {
            Ok(StateUpdate::Full {
                state: self.clone(),
            })
        }
    }

    /// Compute deterministic SHA-256 hash of the semantic root content.
    /// Excludes page_instance_id, timestamp, and load_profile — only the
    /// semantic tree contributes to the hash.
    fn compute_hash(root: &SemanticNode) -> String {
        let mut hasher = Sha256::new();
        Self::hash_node(&mut hasher, root);
        hex::encode(hasher.finalize())
    }

    fn hash_node(hasher: &mut Sha256, node: &SemanticNode) {
        hasher.update(b"role\0");
        hasher.update(node.role.as_bytes());
        hasher.update(b"\0");

        Self::hash_optional_string(hasher, b"label\0", node.label.as_deref());

        hasher.update(b"attributes\0");
        if let Some(attributes) = node.attributes.as_ref() {
            for (key, value) in attributes {
                hasher.update(key.as_bytes());
                hasher.update(b"\0");
                hasher.update(value.as_bytes());
                hasher.update(b"\0");
            }
        }
        hasher.update(b"\x1e");

        Self::hash_optional_string(hasher, b"stable_key\0", node.stable_key.as_deref());

        hasher.update(b"ambiguous\0");
        hasher.update([u8::from(node.ambiguous)]);
        hasher.update(b"\0");

        Self::hash_optional_string(hasher, b"alias\0", node.alias.as_deref());

        hasher.update(b"children\0");
        hasher.update((node.children.len() as u64).to_le_bytes());
        for child in &node.children {
            Self::hash_node(hasher, child);
        }
        hasher.update(b"\x1f");
    }

    fn hash_optional_string(hasher: &mut Sha256, label: &[u8], value: Option<&str>) {
        hasher.update(label);
        match value {
            Some(value) => {
                hasher.update([1]);
                hasher.update(value.as_bytes());
            }
            None => hasher.update([0]),
        }
        hasher.update(b"\0");
    }

    fn generate_fast_state_with_trace(
        &self,
        generation_trace: &mut Vec<StateGenerationPhase>,
    ) -> FastSemanticState {
        generation_trace.push(StateGenerationPhase::Fast);
        self.generate_fast_state()
    }

    fn generate_full_state_with_trace(
        &self,
        _fast: &FastSemanticState,
        generation_trace: &mut Vec<StateGenerationPhase>,
    ) -> FullSemanticState {
        generation_trace.push(StateGenerationPhase::Full);
        self.generate_full_state()
    }
}

fn collect_nodes_matching<F>(root: &SemanticNode, predicate: F) -> Vec<SemanticNode>
where
    F: Fn(&SemanticNode) -> bool,
{
    let mut collected = Vec::new();
    collect_nodes_recursive(root, &predicate, &mut collected);
    collected
}

fn collect_nodes_recursive<F>(node: &SemanticNode, predicate: &F, out: &mut Vec<SemanticNode>)
where
    F: Fn(&SemanticNode) -> bool,
{
    if predicate(node) {
        out.push(project_node_without_children(node));
    }

    for child in &node.children {
        collect_nodes_recursive(child, predicate, out);
    }
}

fn project_node_without_children(node: &SemanticNode) -> SemanticNode {
    SemanticNode {
        role: node.role.clone(),
        label: node.label.clone(),
        children: Vec::new(),
        attributes: node.attributes.clone(),
        stable_key: node.stable_key.clone(),
        ambiguous: node.ambiguous,
        alias: node.alias.clone(),
        backend_node_id: node.backend_node_id,
    }
}

fn is_interactive_node(node: &SemanticNode) -> bool {
    matches!(
        node.role.as_str(),
        "a" | "button" | "input" | "select" | "textarea" | "option" | "summary"
    ) || node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("role"))
        .is_some_and(|role| {
            matches!(
                role.as_str(),
                "button" | "link" | "checkbox" | "radio" | "switch" | "tab" | "menuitem" | "option"
            )
        })
}

fn is_message_node(node: &SemanticNode) -> bool {
    node.role == "text"
        && node
            .label
            .as_deref()
            .is_some_and(|value| !value.trim().is_empty())
}

fn is_form_node(node: &SemanticNode) -> bool {
    matches!(
        node.role.as_str(),
        "form" | "input" | "select" | "textarea" | "button" | "label" | "option"
    )
}

fn is_region_node(node: &SemanticNode) -> bool {
    matches!(
        node.role.as_str(),
        "main" | "nav" | "section" | "article" | "aside" | "header" | "footer"
    ) || node
        .attributes
        .as_ref()
        .and_then(|attrs| attrs.get("role"))
        .is_some_and(|role| {
            matches!(
                role.as_str(),
                "region" | "main" | "navigation" | "complementary" | "banner" | "contentinfo"
            )
        })
}

fn full_update_payload_size_bytes(state: &SemanticState) -> Result<usize> {
    serde_json::to_vec(&StateUpdate::Full {
        state: state.clone(),
    })
    .context("Failed to encode full update payload for policy check")
    .map(|payload| payload.len())
}

fn should_send_delta(
    full_payload_bytes: usize,
    delta: &SemanticDelta,
    policy: DeltaPolicy,
) -> bool {
    if delta.operation_count() == 0 || delta.operation_count() > policy.max_operations {
        return false;
    }

    if full_payload_bytes == 0 {
        return false;
    }

    let patch_ratio = delta.patch_size_bytes() as f32 / full_payload_bytes as f32;
    patch_ratio <= policy.max_patch_bytes_ratio
}
