use super::profile::LoadProfile;
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
    #[serde(skip_serializing_if = "Vec::is_empty")]
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

    /// Compute deterministic SHA-256 hash of the semantic root content.
    /// Excludes page_instance_id, timestamp, and load_profile — only the
    /// semantic tree contributes to the hash.
    fn compute_hash(root: &SemanticNode) -> String {
        // Clone and strip volatile fields (backend_node_id) to ensure deterministic hash.
        // backend_node_id changes across sessions, but state_hash must be stable for same content.
        let mut clean_root = root.clone();
        Self::strip_volatile_fields(&mut clean_root);

        // BTreeMap ensures deterministic map iteration order for serialization
        let json_content = serde_json::to_string(&clean_root).unwrap_or_default();
        let mut hasher = Sha256::new();
        hasher.update(json_content);
        hex::encode(hasher.finalize())
    }

    fn strip_volatile_fields(node: &mut SemanticNode) {
        node.backend_node_id = 0;
        for child in &mut node.children {
            Self::strip_volatile_fields(child);
        }
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
