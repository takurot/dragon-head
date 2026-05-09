use super::profile::LoadProfile;
use super::stable_key::StableKeyGenerator;
use super::state::SemanticNode;
use anyhow::Result;
use headless_chrome::protocol::cdp::DOM::Node;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap, HashSet};

const DEFAULT_VIEWPORT_WIDTH_PX: f64 = 800.0;
const DEFAULT_VIEWPORT_HEIGHT_PX: f64 = 600.0;

/// Explicit viewport dimensions used for quadrant calculation.
/// When provided to `normalize_dom_with_viewport`, the actual browser dimensions
/// are used instead of the default 800×600 fallback constants.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportDimensions {
    pub width: f64,
    pub height: f64,
}

impl Default for ViewportDimensions {
    fn default() -> Self {
        Self {
            width: DEFAULT_VIEWPORT_WIDTH_PX,
            height: DEFAULT_VIEWPORT_HEIGHT_PX,
        }
    }
}

pub fn normalize_dom(profile: LoadProfile, node: &Node) -> Result<SemanticNode> {
    normalize_dom_internal(profile, node, ViewportDimensions::default(), None)
}

/// Normalize DOM using explicit viewport dimensions for accurate quadrant calculation.
///
/// When the real browser viewport is known (e.g. retrieved via CDP), pass it here so
/// that `stable_key` quadrant values reflect actual on-screen positions rather than
/// the 800×600 default fallback.
pub fn normalize_dom_with_viewport(
    profile: LoadProfile,
    node: &Node,
    viewport: ViewportDimensions,
) -> Result<SemanticNode> {
    normalize_dom_internal(profile, node, viewport, None)
}

pub struct SubtreeRefinementConfig<'a> {
    pub dirty_paths: &'a HashSet<String>,
    pub cached_paths: &'a HashMap<String, Vec<usize>>,
    pub cached_root: &'a SemanticNode,
}

pub fn normalize_dom_with_refinement(
    profile: LoadProfile,
    node: &Node,
    refinement: SubtreeRefinementConfig<'_>,
) -> Result<SemanticNode> {
    normalize_dom_internal(
        profile,
        node,
        ViewportDimensions::default(),
        Some(&refinement),
    )
}

/// Normalize DOM with both explicit viewport dimensions and subtree refinement.
///
/// When `viewport` differs from the default (800×600), cached subtrees are **not**
/// reused even for nodes that are not marked dirty.  Cached stable keys were
/// computed against the old viewport, so reusing them after a resize (or mobile
/// emulation) would produce stale quadrant values.  Bypassing the cache in that
/// case is a safe conservative fix: a full re-parse is triggered whenever the
/// viewport is non-default.
pub fn normalize_dom_with_viewport_and_refinement(
    profile: LoadProfile,
    node: &Node,
    viewport: ViewportDimensions,
    refinement: SubtreeRefinementConfig<'_>,
) -> Result<SemanticNode> {
    if viewport != ViewportDimensions::default() {
        // Viewport-sensitive path: skip the refinement cache entirely so that
        // all stable keys are recomputed with the current viewport dimensions.
        return normalize_dom_internal(profile, node, viewport, None);
    }
    normalize_dom_internal(profile, node, viewport, Some(&refinement))
}

fn normalize_dom_internal(
    profile: LoadProfile,
    node: &Node,
    viewport: ViewportDimensions,
    refinement: Option<&SubtreeRefinementConfig<'_>>,
) -> Result<SemanticNode> {
    let mut key_generator = StableKeyGenerator::new();
    // Start traversal with root path
    let internal_node = traverse_node(
        profile,
        node,
        &mut key_generator,
        "root",
        viewport,
        refinement,
    )?;
    Ok(internal_node.unwrap_or_default())
}

/// Returns true if a CSS class token looks like a dynamic/generated hash.
/// Heuristic: a class is "dynamic" if it contains a hex-like or base64-like
/// substring of 6+ characters with mixed digits and letters, or is purely
/// alphanumeric with length >= 8 and contains at least 3 digits.
fn is_dynamic_class(class: &str) -> bool {
    if class.len() < 6 {
        return false;
    }
    let digit_count = class.chars().filter(|c| c.is_ascii_digit()).count();
    let alpha_count = class.chars().filter(|c| c.is_ascii_alphabetic()).count();

    // Pure hex-like hash (e.g. "a1b2c3d4", "css-1a2b3c")
    if class.len() >= 8 && digit_count >= 3 && alpha_count >= 2 {
        return true;
    }
    // Short hash with high digit ratio (e.g. "sc-12345", "emotion-abc123")
    if digit_count as f64 / class.len() as f64 > 0.4 {
        return true;
    }
    false
}

/// Filter dynamic classes from a space-separated class string.
/// Returns None if all classes were dynamic (nothing left).
fn filter_dynamic_classes(class_value: &str) -> Option<String> {
    let kept: Vec<&str> = class_value
        .split_whitespace()
        .filter(|cls| !is_dynamic_class(cls))
        .collect();
    if kept.is_empty() {
        None
    } else {
        Some(kept.join(" "))
    }
}

fn traverse_node(
    profile: LoadProfile,
    node: &Node,
    key_gen: &mut StableKeyGenerator,
    parent_path: &str,
    viewport: ViewportDimensions,
    refinement: Option<&SubtreeRefinementConfig<'_>>,
) -> Result<Option<SemanticNode>> {
    // Basic filtering based on node type
    // NodeType: 1=Element, 3=Text, 9=Document
    let node_type = node.node_type;
    let node_name = node.node_name.to_lowercase();

    if node_type == 3 {
        // Text node
        let text = node.node_value.clone();
        if text.trim().is_empty() {
            return Ok(None);
        }

        let (stable_key, ambiguous) =
            key_gen.generate_key("text", Some(&text), parent_path, "unknown");

        return Ok(Some(SemanticNode {
            role: "text".to_string(),
            label: Some(text),
            stable_key: Some(stable_key),
            ambiguous,
            backend_node_id: node.backend_node_id.into(),
            ..Default::default()
        }));
    }

    if node_type != 1 && node_type != 9 {
        return Ok(None); // Skip comments, etc.
    }

    let current_path = format!("{}/{}", parent_path, node_name);

    if let Some(cached) = resolve_cached_subtree(&current_path, &node_name, refinement) {
        return Ok(Some(cached.clone()));
    }

    // Filter by tag name based on Load Profile (SPEC SRE-01)
    // Minimal: text/structure only — block images, video, ads, analytics JS
    // Visual:  allow images for SoM generation — block JS, video, ads
    // Interactive: allow essential JS for SPA/login — block ads/analytics only
    match profile {
        LoadProfile::Minimal => match node_name.as_str() {
            "script" | "style" | "img" | "video" | "svg" | "iframe" | "noscript" | "meta"
            | "link" | "canvas" | "audio" | "source" | "picture" => return Ok(None),
            _ => {}
        },
        LoadProfile::Visual => match node_name.as_str() {
            "script" | "style" | "video" | "iframe" | "noscript" | "meta" | "link" | "canvas"
            | "audio" => return Ok(None),
            // img, svg, picture, source are ALLOWED for visual rendering
            _ => {}
        },
        LoadProfile::Interactive => match node_name.as_str() {
            // Only block non-essential elements — allow script for SPA
            "noscript" | "meta" | "link" => return Ok(None),
            _ => {}
        },
    }

    // Filter by role="presentation" (Ads/Layout)
    // Attributes in headless_chrome are Vec<String> [key1, val1, key2, val2...]
    let mut attributes = BTreeMap::new();
    if let Some(attrs) = &node.attributes {
        for chunk in attrs.chunks(2) {
            if chunk.len() == 2 {
                let key = &chunk[0];
                let val = &chunk[1];

                if key == "role" && val == "presentation" {
                    return Ok(None);
                }

                // SPEC SRE-01: "動的クラス名（ハッシュ値）の削除"
                // Strip dynamic/generated class tokens to ensure deterministic hashing.
                if key == "class" {
                    if let Some(filtered) = filter_dynamic_classes(val) {
                        attributes.insert(key.clone(), filtered);
                    }
                    // If all classes were dynamic, skip the attribute entirely
                    continue;
                }

                attributes.insert(key.clone(), val.clone());
            }
        }

        if node_name == "input" {
            if let Some(t) = attributes.get("type") {
                if (t == "password" || t == "email") && attributes.contains_key("value") {
                    attributes.insert("value".to_string(), "***".to_string());
                }
            }
        }
    }

    let mut children = Vec::new();
    if let Some(child_nodes) = &node.children {
        // We need to track sibling index per role? Or just absolute index?
        // Using absolute index for simplicity in traversal, but for stable keys relying on structure,
        // we might want "nth-of-type".
        // For now, let's use the loop index.
        for child in child_nodes {
            if let Some(normalized_child) =
                traverse_node(profile, child, key_gen, &current_path, viewport, refinement)?
            {
                children.push(normalized_child);
            }
        }
    }

    // Generate stable key for this element
    // Label for element? Maybe id, or aria-label, or text content?
    // For now, we use a simple approach: if it has an id, use it as label hint?
    // Or just empty label for container elements.
    // Ideally we should extract text content if it's a leaf interactive element.
    let text_hint = extract_direct_text_label(node);
    let label_hint = attributes
        .get("aria-label")
        .map(|s| s.as_str())
        .or_else(|| attributes.get("title").map(|s| s.as_str()))
        .or_else(|| attributes.get("id").map(|s| s.as_str()))
        .or(text_hint.as_deref());

    let quadrant = resolve_quadrant(&attributes, &node_name, label_hint, parent_path, viewport);
    let alias = build_alias(&node_name, label_hint, &attributes);

    let (stable_key, ambiguous) =
        key_gen.generate_key(&node_name, label_hint, parent_path, &quadrant);

    Ok(Some(SemanticNode {
        role: node_name,
        label: label_hint.map(|s| s.to_string()),
        children,
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
        stable_key: Some(stable_key),
        ambiguous,
        alias,
        backend_node_id: node.backend_node_id.into(),
    }))
}

fn resolve_cached_subtree<'a>(
    current_path: &str,
    role: &str,
    refinement: Option<&'a SubtreeRefinementConfig<'_>>,
) -> Option<&'a SemanticNode> {
    let refinement = refinement?;
    if path_overlaps_dirty_set(current_path, refinement.dirty_paths) {
        return None;
    }
    let index_path = refinement.cached_paths.get(current_path)?;
    let cached = node_by_child_index_path(refinement.cached_root, index_path)?;
    if cached.role == role {
        Some(cached)
    } else {
        None
    }
}

fn node_by_child_index_path<'a>(
    root: &'a SemanticNode,
    child_index_path: &[usize],
) -> Option<&'a SemanticNode> {
    let mut current = root;
    for child_index in child_index_path {
        current = current.children.get(*child_index)?;
    }
    Some(current)
}

fn path_overlaps_dirty_set(path: &str, dirty_paths: &HashSet<String>) -> bool {
    dirty_paths.iter().any(|dirty| {
        dirty == path || is_descendant_path(path, dirty) || is_descendant_path(dirty, path)
    })
}

fn is_descendant_path(path: &str, ancestor: &str) -> bool {
    if path.len() <= ancestor.len() || !path.starts_with(ancestor) {
        return false;
    }
    path.as_bytes()
        .get(ancestor.len())
        .is_some_and(|separator| *separator == b'/')
}

fn extract_direct_text_label(node: &Node) -> Option<String> {
    let children = node.children.as_ref()?;
    let mut parts = Vec::new();

    for child in children {
        if child.node_type != 3 {
            continue;
        }
        let text = child.node_value.trim();
        if !text.is_empty() {
            parts.push(crate::privacy::global().redact_text(text));
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn resolve_quadrant(
    attributes: &BTreeMap<String, String>,
    role: &str,
    label_hint: Option<&str>,
    dom_signature: &str,
    viewport: ViewportDimensions,
) -> String {
    if let Some(raw) = attributes.get("data-quadrant") {
        if let Some(normalized) = normalize_quadrant_token(raw) {
            return normalized;
        }
    }

    if let Some(quadrant) = resolve_quadrant_from_coordinates(attributes, viewport) {
        return quadrant;
    }

    resolve_quadrant_from_signature(role, label_hint, dom_signature)
}

fn normalize_quadrant_token(raw: &str) -> Option<String> {
    let normalized = slugify(raw).replace('-', "_");
    match normalized.as_str() {
        "top_left" | "tl" => Some("top_left".to_string()),
        "top_right" | "tr" => Some("top_right".to_string()),
        "bottom_left" | "bl" => Some("bottom_left".to_string()),
        "bottom_right" | "br" => Some("bottom_right".to_string()),
        _ => None,
    }
}

fn resolve_quadrant_from_coordinates(
    attributes: &BTreeMap<String, String>,
    viewport: ViewportDimensions,
) -> Option<String> {
    let x_ratio = extract_x_ratio(attributes, viewport.width)?;
    let y_ratio = extract_y_ratio(attributes, viewport.height)?;
    Some(quadrant_from_ratios(x_ratio, y_ratio))
}

fn extract_x_ratio(attributes: &BTreeMap<String, String>, viewport_width: f64) -> Option<f64> {
    if let Some(style) = attributes.get("style") {
        if let Some(left) =
            extract_css_numeric(style, "left").and_then(|v| v.to_ratio(viewport_width))
        {
            return Some(left);
        }
        if let Some(right) =
            extract_css_numeric(style, "right").and_then(|v| v.to_ratio(viewport_width))
        {
            return Some((1.0 - right).clamp(0.0, 1.0));
        }
    }

    extract_coordinate_attribute(attributes, &["data-x", "x"], viewport_width)
}

fn extract_y_ratio(attributes: &BTreeMap<String, String>, viewport_height: f64) -> Option<f64> {
    if let Some(style) = attributes.get("style") {
        if let Some(top) =
            extract_css_numeric(style, "top").and_then(|v| v.to_ratio(viewport_height))
        {
            return Some(top);
        }
        if let Some(bottom) =
            extract_css_numeric(style, "bottom").and_then(|v| v.to_ratio(viewport_height))
        {
            return Some((1.0 - bottom).clamp(0.0, 1.0));
        }
    }

    extract_coordinate_attribute(attributes, &["data-y", "y"], viewport_height)
}

fn extract_coordinate_attribute(
    attributes: &BTreeMap<String, String>,
    names: &[&str],
    axis_px: f64,
) -> Option<f64> {
    names.iter().find_map(|name| {
        let value = attributes.get(*name)?;
        parse_css_numeric(value).and_then(|coord| coord.to_ratio(axis_px))
    })
}

fn extract_css_numeric(style: &str, property: &str) -> Option<CssNumeric> {
    style
        .split(';')
        .filter_map(|entry| entry.split_once(':'))
        .find_map(|(key, value)| {
            if key.trim().eq_ignore_ascii_case(property) {
                parse_css_numeric(value.trim())
            } else {
                None
            }
        })
}

fn parse_css_numeric(raw: &str) -> Option<CssNumeric> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }

    if let Some(percent) = value.strip_suffix('%') {
        return percent.trim().parse::<f64>().ok().map(CssNumeric::Percent);
    }

    if let Some(px) = value.strip_suffix("px") {
        return px.trim().parse::<f64>().ok().map(CssNumeric::Px);
    }

    value.parse::<f64>().ok().map(CssNumeric::Px)
}

fn resolve_quadrant_from_signature(
    role: &str,
    label_hint: Option<&str>,
    dom_signature: &str,
) -> String {
    let normalized_label = label_hint.unwrap_or("").trim().to_lowercase();
    let signature = format!("{role}|{normalized_label}|{dom_signature}");
    let digest = Sha256::digest(signature.as_bytes());
    match digest[0] % 4 {
        0 => "top_left".to_string(),
        1 => "top_right".to_string(),
        2 => "bottom_left".to_string(),
        _ => "bottom_right".to_string(),
    }
}

fn quadrant_from_ratios(x_ratio: f64, y_ratio: f64) -> String {
    if y_ratio < 0.5 {
        if x_ratio < 0.5 {
            "top_left".to_string()
        } else {
            "top_right".to_string()
        }
    } else if x_ratio < 0.5 {
        "bottom_left".to_string()
    } else {
        "bottom_right".to_string()
    }
}

#[derive(Clone, Copy, Debug)]
enum CssNumeric {
    Px(f64),
    Percent(f64),
}

impl CssNumeric {
    fn to_ratio(self, axis_px: f64) -> Option<f64> {
        if axis_px <= 0.0 {
            return None;
        }

        let ratio = match self {
            CssNumeric::Px(value) => value / axis_px,
            CssNumeric::Percent(value) => value / 100.0,
        };
        Some(ratio.clamp(0.0, 1.0))
    }
}

fn build_alias(
    role: &str,
    label_hint: Option<&str>,
    attributes: &BTreeMap<String, String>,
) -> Option<String> {
    if let Some(raw) = attributes.get("data-alias") {
        let normalized = slugify(raw);
        if !normalized.is_empty() {
            return Some(normalized);
        }
    }

    let label = label_hint
        .or_else(|| attributes.get("name").map(|s| s.as_str()))
        .or_else(|| attributes.get("type").map(|s| s.as_str()));

    let role_part = slugify(role);
    if role_part.is_empty() {
        return None;
    }

    let label_part = label.map(slugify).unwrap_or_default();
    if label_part.is_empty() {
        Some(role_part)
    } else {
        Some(format!("{}_{}", role_part, label_part))
    }
}

fn slugify(input: &str) -> String {
    let mut normalized = String::with_capacity(input.len());
    let mut last_was_separator = false;

    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() {
            normalized.push(ch.to_ascii_lowercase());
            last_was_separator = false;
            continue;
        }

        if !last_was_separator {
            normalized.push('_');
            last_was_separator = true;
        }
    }

    normalized.trim_matches('_').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use headless_chrome::protocol::cdp::DOM::Node;
    use serde_json::json;
    use std::collections::{HashMap, HashSet};

    #[test]
    fn test_is_dynamic_class() {
        // Dynamic (hash-like)
        assert!(is_dynamic_class("sc-12345abc"));
        assert!(is_dynamic_class("css-1a2b3c4d"));
        assert!(is_dynamic_class("emotion-abc123"));
        assert!(is_dynamic_class("a1b2c3d4e5"));

        // Static (semantic)
        assert!(!is_dynamic_class("btn"));
        assert!(!is_dynamic_class("primary"));
        assert!(!is_dynamic_class("container"));
        assert!(!is_dynamic_class("nav-item"));
        assert!(!is_dynamic_class("col-md"));
    }

    #[test]
    fn test_filter_dynamic_classes() {
        // Mix of static and dynamic
        assert_eq!(
            filter_dynamic_classes("btn sc-12345abc primary"),
            Some("btn primary".to_string())
        );
        // All dynamic
        assert_eq!(filter_dynamic_classes("sc-12345abc css-a1b2c3d4"), None);
        // All static
        assert_eq!(
            filter_dynamic_classes("btn primary"),
            Some("btn primary".to_string())
        );
    }

    #[test]
    fn test_path_overlaps_dirty_set_detects_ancestor_or_descendant() {
        let dirty_paths = HashSet::from([
            "root/#document/html/body/div".to_string(),
            "root/#document/html/body/section/button".to_string(),
        ]);

        assert!(path_overlaps_dirty_set(
            "root/#document/html/body/div/span",
            &dirty_paths
        ));
        assert!(path_overlaps_dirty_set(
            "root/#document/html/body",
            &dirty_paths
        ));
        assert!(path_overlaps_dirty_set(
            "root/#document/html/body/section/button",
            &dirty_paths
        ));
        assert!(!path_overlaps_dirty_set(
            "root/#document/html/head/title",
            &dirty_paths
        ));
    }

    #[test]
    fn test_is_descendant_path_requires_segment_boundary() {
        assert!(is_descendant_path(
            "root/#document/html/body/div/span",
            "root/#document/html/body/div"
        ));
        assert!(!is_descendant_path(
            "root/#document/html/body/division",
            "root/#document/html/body/div"
        ));
    }

    #[test]
    fn test_refinement_reuses_clean_subtree_and_reparses_dirty_subtree() -> Result<()> {
        let previous_dom = build_document_fixture("left_v1", "right_v1", 100)?;
        let previous = normalize_dom(LoadProfile::Minimal, &previous_dom)?;
        let cached_paths = semantic_path_index_fixture();
        let dirty_paths = HashSet::from(["root/#document/html/body/section".to_string()]);

        // DIV changed too, but only SECTION is marked dirty.
        // The clean DIV path should be reused from cached subtree.
        let next_dom = build_document_fixture("left_v2_should_be_ignored", "right_v2", 500)?;
        let refined = normalize_dom_with_refinement(
            LoadProfile::Minimal,
            &next_dom,
            SubtreeRefinementConfig {
                dirty_paths: &dirty_paths,
                cached_paths: &cached_paths,
                cached_root: &previous,
            },
        )?;

        let body = &refined.children[0].children[0];
        let div = &body.children[0];
        let section = &body.children[1];

        assert_eq!(div.label.as_deref(), Some("left_v1"));
        assert_eq!(div.backend_node_id, 104);
        assert_eq!(section.label.as_deref(), Some("right_v2"));
        assert_eq!(section.backend_node_id, 506);

        Ok(())
    }

    #[test]
    fn test_refinement_reparses_descendants_when_ancestor_is_dirty() -> Result<()> {
        let previous_dom = build_document_fixture("left_v1", "right_v1", 100)?;
        let previous = normalize_dom(LoadProfile::Minimal, &previous_dom)?;
        let cached_paths = semantic_path_index_fixture();
        let dirty_paths = HashSet::from(["root/#document/html/body".to_string()]);
        let next_dom = build_document_fixture("left_v2", "right_v2", 500)?;

        let refined = normalize_dom_with_refinement(
            LoadProfile::Minimal,
            &next_dom,
            SubtreeRefinementConfig {
                dirty_paths: &dirty_paths,
                cached_paths: &cached_paths,
                cached_root: &previous,
            },
        )?;

        let body = &refined.children[0].children[0];
        let div = &body.children[0];
        let section = &body.children[1];

        assert_eq!(div.label.as_deref(), Some("left_v2"));
        assert_eq!(div.backend_node_id, 504);
        assert_eq!(section.label.as_deref(), Some("right_v2"));
        assert_eq!(section.backend_node_id, 506);

        Ok(())
    }

    #[test]
    fn test_pii_redaction() -> Result<()> {
        let password_input = make_element_node(
            104,
            "input",
            vec![("type", "password"), ("value", "MySecretP@ssw0rd!")],
            vec![],
        )?;
        let email_input = make_element_node(
            105,
            "input",
            vec![("type", "email"), ("value", "test@example.com")],
            vec![],
        )?;
        let safe_input = make_element_node(
            106,
            "input",
            vec![("type", "text"), ("value", "Hello World")],
            vec![],
        )?;

        let cc_text = make_text_node(107, "Here is my card: 1234-5678-9012-3456 please charge it")?;
        let cc_container = make_element_node(108, "div", vec![], vec![cc_text])?;

        // ISSUE-08: email addresses in text nodes must also be masked
        let email_text = make_text_node(109, "Contact us at support@example.com for help")?;
        let email_container = make_element_node(110, "div", vec![], vec![email_text])?;

        let body = make_element_node(
            103,
            "body",
            vec![],
            vec![
                password_input,
                email_input,
                safe_input,
                cc_container,
                email_container,
            ],
        )?;
        let html = make_element_node(102, "html", vec![], vec![body])?;
        let dom = make_document_node(101, vec![html])?;

        let sem_node = normalize_dom(LoadProfile::Minimal, &dom)?;

        let body_node = &sem_node.children[0].children[0];

        // Check password output
        let pass_attr = body_node.children[0].attributes.as_ref().unwrap();
        assert_eq!(pass_attr.get("value").unwrap(), "***");

        // Check email output
        let email_attr = body_node.children[1].attributes.as_ref().unwrap();
        assert_eq!(email_attr.get("value").unwrap(), "***");

        // Check safe text output
        let safe_attr = body_node.children[2].attributes.as_ref().unwrap();
        assert_eq!(safe_attr.get("value").unwrap(), "Hello World");

        // Check text redact output
        let cc_extracted_label = body_node.children[3].label.as_deref().unwrap();
        assert_eq!(
            cc_extracted_label,
            "Here is my card: ****-****-****-XXXX please charge it"
        );

        // ISSUE-08: email addresses in text nodes must be masked (was missing before fix)
        let email_text_label = body_node.children[4].label.as_deref().unwrap();
        assert_eq!(email_text_label, "Contact us at *** for help");

        Ok(())
    }

    fn semantic_path_index_fixture() -> HashMap<String, Vec<usize>> {
        HashMap::from([
            ("root/#document".to_string(), vec![]),
            ("root/#document/html".to_string(), vec![0]),
            ("root/#document/html/body".to_string(), vec![0, 0]),
            ("root/#document/html/body/div".to_string(), vec![0, 0, 0]),
            (
                "root/#document/html/body/section".to_string(),
                vec![0, 0, 1],
            ),
        ])
    }

    fn build_document_fixture(left_text: &str, right_text: &str, id_base: u32) -> Result<Node> {
        let div = make_element_node(
            id_base + 4,
            "div",
            vec![],
            vec![make_text_node(id_base + 5, left_text)?],
        )?;
        let section = make_element_node(
            id_base + 6,
            "section",
            vec![],
            vec![make_text_node(id_base + 7, right_text)?],
        )?;
        let body = make_element_node(id_base + 3, "body", vec![], vec![div, section])?;
        let html = make_element_node(id_base + 2, "html", vec![], vec![body])?;
        make_document_node(id_base + 1, vec![html])
    }

    fn make_document_node(node_id: u32, children: Vec<Node>) -> Result<Node> {
        Ok(serde_json::from_value(json!({
            "nodeId": node_id,
            "backendNodeId": node_id,
            "nodeType": 9,
            "nodeName": "#document",
            "localName": "",
            "nodeValue": "",
            "childNodeCount": children.len(),
            "children": children
        }))?)
    }

    fn make_element_node(
        node_id: u32,
        tag: &str,
        attributes: Vec<(&str, &str)>,
        children: Vec<Node>,
    ) -> Result<Node> {
        let attrs = flatten_attrs(attributes);
        Ok(serde_json::from_value(json!({
            "nodeId": node_id,
            "backendNodeId": node_id,
            "nodeType": 1,
            "nodeName": tag.to_uppercase(),
            "localName": tag,
            "nodeValue": "",
            "attributes": attrs,
            "childNodeCount": children.len(),
            "children": children
        }))?)
    }

    fn make_text_node(node_id: u32, text: &str) -> Result<Node> {
        Ok(serde_json::from_value(json!({
            "nodeId": node_id,
            "backendNodeId": node_id,
            "nodeType": 3,
            "nodeName": "#text",
            "localName": "",
            "nodeValue": text
        }))?)
    }

    fn flatten_attrs(attributes: Vec<(&str, &str)>) -> Vec<String> {
        let mut flat = Vec::with_capacity(attributes.len() * 2);
        for (key, value) in attributes {
            flat.push(key.to_string());
            flat.push(value.to_string());
        }
        flat
    }
}
