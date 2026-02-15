use super::profile::LoadProfile;
use super::stable_key::StableKeyGenerator;
use super::state::SemanticNode;
use anyhow::Result;
use headless_chrome::protocol::cdp::DOM::Node;
use std::collections::BTreeMap;

pub fn normalize_dom(profile: LoadProfile, node: &Node) -> Result<SemanticNode> {
    let mut key_generator = StableKeyGenerator::new();
    // Start traversal with root path
    let internal_node = traverse_node(profile, node, &mut key_generator, "root")?;
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
    }

    // Generate path for this node to pass to children
    // path format: parent_path/role (without index to ensure stability on sibling insertion, unless collision happens in key gen)
    let current_path = format!("{}/{}", parent_path, node_name);

    let mut children = Vec::new();
    if let Some(child_nodes) = &node.children {
        // We need to track sibling index per role? Or just absolute index?
        // Using absolute index for simplicity in traversal, but for stable keys relying on structure,
        // we might want "nth-of-type".
        // For now, let's use the loop index.
        for child in child_nodes {
            if let Some(normalized_child) = traverse_node(profile, child, key_gen, &current_path)? {
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

    let quadrant = resolve_quadrant(&attributes);
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

fn extract_direct_text_label(node: &Node) -> Option<String> {
    let children = node.children.as_ref()?;
    let mut parts = Vec::new();

    for child in children {
        if child.node_type != 3 {
            continue;
        }
        let text = child.node_value.trim();
        if !text.is_empty() {
            parts.push(text.to_string());
        }
    }

    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" "))
    }
}

fn resolve_quadrant(attributes: &BTreeMap<String, String>) -> String {
    if let Some(raw) = attributes.get("data-quadrant") {
        let normalized = slugify(raw);
        if !normalized.is_empty() {
            return normalized;
        }
    }

    if let Some(style) = attributes.get("style") {
        if let (Some(left), Some(top)) = (
            extract_percentage_from_style(style, "left"),
            extract_percentage_from_style(style, "top"),
        ) {
            if top < 50.0 {
                return if left < 50.0 {
                    "top_left".to_string()
                } else {
                    "top_right".to_string()
                };
            }
            return if left < 50.0 {
                "bottom_left".to_string()
            } else {
                "bottom_right".to_string()
            };
        }
    }

    "unknown".to_string()
}

fn extract_percentage_from_style(style: &str, property: &str) -> Option<f64> {
    style
        .split(';')
        .filter_map(|entry| entry.split_once(':'))
        .find_map(|(key, value)| {
            if key.trim().eq_ignore_ascii_case(property) {
                value.trim().strip_suffix('%')?.trim().parse::<f64>().ok()
            } else {
                None
            }
        })
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
}
