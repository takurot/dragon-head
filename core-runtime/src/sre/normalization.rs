use super::profile::LoadProfile;
use super::state::SemanticNode;
use anyhow::Result;
use headless_chrome::protocol::cdp::DOM::Node;
use std::collections::BTreeMap;

pub fn normalize_dom(profile: LoadProfile, node: &Node) -> Result<SemanticNode> {
    let internal_node = traverse_node(profile, node)?;
    Ok(internal_node.unwrap_or_default())
}

fn traverse_node(profile: LoadProfile, node: &Node) -> Result<Option<SemanticNode>> {
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
        return Ok(Some(SemanticNode {
            role: "text".to_string(),
            label: Some(text),
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

                // Dynamic class filtering (heuristic: heuristic for now - maybe just skip class completely for minimal?)
                // Spec SRE-01: "normalize: dynamic class removal"
                if key == "class" {
                    // Simple heuristic: if class contains numbers > 4 digits or is super long hash?
                    // For now, let's keep clean classes.
                    // Or just keep it as is, and rely on hashing?
                    // Spec says "dynamic class removal".
                    // Let's implement a dummy filter (passing everything for now unless it looks like a hash)
                    // Actually, if we want deterministic hash across reloads where class changes, we MUST strip it.
                    // If we don't know which class is dynamic, stripping ALL classes is safer for SRE.
                    // But classes carry semantic meaning (e.g. "btn-primary").
                    // Let's filter classes that look like hashes (alphanumeric, no separators, long).
                } else if key == "role" && val == "presentation" {
                    return Ok(None);
                }

                attributes.insert(key.clone(), val.clone());
            }
        }
    }

    let mut children = Vec::new();
    if let Some(child_nodes) = &node.children {
        for child in child_nodes {
            if let Some(normalized_child) = traverse_node(profile, child)? {
                children.push(normalized_child);
            }
        }
    }

    Ok(Some(SemanticNode {
        role: node_name, // Use tag name as role for now
        children,
        attributes: if attributes.is_empty() {
            None
        } else {
            Some(attributes)
        },
        ..Default::default()
    }))
}
