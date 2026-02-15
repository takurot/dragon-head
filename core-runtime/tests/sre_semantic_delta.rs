use core_runtime::sre::{DeltaPolicy, LoadProfile, SemanticNode, SemanticState, StateUpdate};

fn build_catalog_state(changed_index: usize, changed_suffix: &str) -> SemanticState {
    let mut children = Vec::new();

    for idx in 0..40 {
        let label = if idx == changed_index {
            format!("Product {idx:02} {changed_suffix}")
        } else {
            format!("Product {idx:02}")
        };

        children.push(SemanticNode {
            role: "button".to_string(),
            label: Some(label),
            stable_key: Some(format!("btn-{idx:02}")),
            backend_node_id: 1_000 + idx as i64,
            ..Default::default()
        });
    }

    let root = SemanticNode {
        role: "body".to_string(),
        label: Some("catalog".to_string()),
        children,
        stable_key: Some("root-catalog".to_string()),
        backend_node_id: 10,
        ..Default::default()
    };

    SemanticState::new(root, LoadProfile::Minimal)
}

fn build_checkout_state() -> SemanticState {
    let root = SemanticNode {
        role: "body".to_string(),
        label: Some("checkout".to_string()),
        children: vec![
            SemanticNode {
                role: "form".to_string(),
                stable_key: Some("checkout-form".to_string()),
                backend_node_id: 2001,
                ..Default::default()
            },
            SemanticNode {
                role: "button".to_string(),
                label: Some("Pay now".to_string()),
                stable_key: Some("pay-now".to_string()),
                backend_node_id: 2002,
                ..Default::default()
            },
        ],
        stable_key: Some("root-checkout".to_string()),
        backend_node_id: 20,
        ..Default::default()
    };

    SemanticState::new(root, LoadProfile::Minimal)
}

#[test]
fn test_semantic_delta_patch_is_smaller_for_minor_change() -> anyhow::Result<()> {
    let previous = build_catalog_state(usize::MAX, "");
    let next = build_catalog_state(22, "updated");

    let delta = next
        .build_delta(&previous)?
        .expect("Minor change should produce a patch");

    let patch_bytes = delta.patch_size_bytes();
    let full_bytes = serde_json::to_vec(next.root())?.len();

    assert!(
        patch_bytes < full_bytes,
        "Patch should be smaller than full state payload (patch={patch_bytes}, full={full_bytes})"
    );

    Ok(())
}

#[test]
fn test_semantic_delta_patch_rebuilds_next_state() -> anyhow::Result<()> {
    let previous = build_catalog_state(usize::MAX, "");
    let next = build_catalog_state(22, "updated");

    let delta = next
        .build_delta(&previous)?
        .expect("Minor change should produce a patch");

    let rebuilt_root = delta.apply_to_root(previous.root())?;
    assert_eq!(rebuilt_root, *next.root());
    assert_eq!(delta.next_state_hash, next.state_hash());

    Ok(())
}

#[test]
fn test_state_update_switches_between_full_and_delta() -> anyhow::Result<()> {
    let baseline = build_catalog_state(usize::MAX, "");
    let minor_change = build_catalog_state(22, "updated");
    let major_change = build_checkout_state();

    let no_history = minor_change.select_update(None, DeltaPolicy::default())?;
    assert!(matches!(no_history, StateUpdate::Full { .. }));

    let for_minor = minor_change.select_update(Some(&baseline), DeltaPolicy::default())?;
    assert!(matches!(for_minor, StateUpdate::Delta { .. }));

    let strict_policy = DeltaPolicy {
        max_patch_bytes_ratio: 1.0,
        max_operations: 1,
    };
    let for_major = major_change.select_update(Some(&baseline), strict_policy)?;
    assert!(matches!(for_major, StateUpdate::Full { .. }));

    Ok(())
}
