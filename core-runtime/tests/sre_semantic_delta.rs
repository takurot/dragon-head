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

fn build_compact_state(cta_label: &str) -> SemanticState {
    let root = SemanticNode {
        role: "body".to_string(),
        children: vec![SemanticNode {
            role: "button".to_string(),
            label: Some(cta_label.to_string()),
            stable_key: Some("compact-cta".to_string()),
            backend_node_id: 3001,
            ..Default::default()
        }],
        stable_key: Some("compact-root".to_string()),
        backend_node_id: 30,
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
    let full_bytes = serde_json::to_vec(&StateUpdate::Full {
        state: next.clone(),
    })?
    .len();

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

#[test]
fn test_state_update_returns_noop_when_state_hash_is_unchanged() -> anyhow::Result<()> {
    let baseline = build_catalog_state(usize::MAX, "");
    let unchanged = build_catalog_state(usize::MAX, "");

    let update = unchanged.select_update(Some(&baseline), DeltaPolicy::default())?;
    match update {
        StateUpdate::Noop { state_hash } => {
            assert_eq!(state_hash, unchanged.state_hash());
        }
        other => panic!("Expected no-op update, got {other:?}"),
    }

    Ok(())
}

#[test]
fn test_state_update_delta_ratio_uses_full_update_payload() -> anyhow::Result<()> {
    let previous = build_compact_state("Continue");
    let next = build_compact_state("Continue securely");
    let delta = next
        .build_delta(&previous)?
        .expect("Label update should produce a patch");

    let patch_bytes = delta.patch_size_bytes() as f32;
    let root_bytes = serde_json::to_vec(next.root())?.len() as f32;
    let full_update_bytes = serde_json::to_vec(&StateUpdate::Full {
        state: next.clone(),
    })?
    .len() as f32;
    let ratio_against_root = patch_bytes / root_bytes;
    let ratio_against_full_update = patch_bytes / full_update_bytes;

    assert!(
        ratio_against_full_update < ratio_against_root,
        "Full update payload should be larger than root-only payload"
    );

    let threshold_between = (ratio_against_full_update + ratio_against_root) / 2.0;
    let update = next.select_update(
        Some(&previous),
        DeltaPolicy {
            max_patch_bytes_ratio: threshold_between,
            max_operations: usize::MAX,
        },
    )?;

    assert!(
        matches!(update, StateUpdate::Delta { .. }),
        "Delta should be selected when threshold is above full-payload ratio"
    );

    Ok(())
}
