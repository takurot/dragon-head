use std::{
    thread,
    time::{Duration, Instant},
};

use core_runtime::sre::{
    AsyncPipeline, AsyncPipelineConfig, AuditEvent, LoadProfile, SemanticNode, SemanticState,
};

fn build_state(label: &str) -> SemanticState {
    let root = SemanticNode {
        role: "body".to_string(),
        children: vec![
            SemanticNode {
                role: "button".to_string(),
                label: Some(format!("Proceed {label}")),
                stable_key: Some(format!("btn-{label}")),
                backend_node_id: 100,
                ..Default::default()
            },
            SemanticNode {
                role: "section".to_string(),
                attributes: Some(std::collections::BTreeMap::from([(
                    "role".to_string(),
                    "region".to_string(),
                )])),
                backend_node_id: 101,
                ..Default::default()
            },
        ],
        stable_key: Some(format!("root-{label}")),
        backend_node_id: 1,
        ..Default::default()
    };

    SemanticState::new(root, LoadProfile::Minimal)
}

#[test]
fn test_async_pipeline_prioritizes_fast_state_over_full_backlog() -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig {
        render_queue_capacity: 8,
        sre_queue_capacity: 8,
        audit_queue_capacity: 8,
        full_stage_delay: Duration::from_millis(150),
        ..Default::default()
    });

    let first = pipeline.submit_state(build_state("first"))?;
    let second = pipeline.submit_state(build_state("second"))?;

    thread::sleep(Duration::from_millis(30));
    first.recv_fast(Duration::from_secs(1))?;

    let started = Instant::now();
    second.recv_fast(Duration::from_secs(1))?;
    let fast_elapsed = started.elapsed();

    assert!(
        fast_elapsed < Duration::from_millis(120),
        "Fast state should not be blocked by queued full-state work (elapsed={fast_elapsed:?})"
    );

    first.recv_full(Duration::from_secs(2))?;
    second.recv_full(Duration::from_secs(2))?;
    Ok(())
}

#[test]
fn test_async_pipeline_applies_backpressure_on_audit_queue() -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig {
        render_queue_capacity: 4,
        sre_queue_capacity: 4,
        audit_queue_capacity: 1,
        audit_stage_delay: Duration::from_millis(250),
        ..Default::default()
    });

    let mut saw_backpressure = false;
    for operation in ["act", "verify", "wait", "extract", "confirm"] {
        if pipeline
            .submit_audit_event(AuditEvent::tool_call(operation))
            .is_err()
        {
            saw_backpressure = true;
            break;
        }
    }

    assert!(
        saw_backpressure,
        "At least one audit event should be rejected by bounded queue backpressure"
    );

    Ok(())
}

#[test]
fn test_async_pipeline_handles_load_without_deadlock() -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig {
        render_queue_capacity: 16,
        sre_queue_capacity: 16,
        audit_queue_capacity: 16,
        ..Default::default()
    });

    let mut handles = Vec::new();
    for idx in 0..80 {
        loop {
            match pipeline.submit_state(build_state(&format!("bulk-{idx}"))) {
                Ok(handle) => {
                    handles.push(handle);
                    break;
                }
                Err(_) => thread::sleep(Duration::from_millis(2)),
            }
        }
    }

    for handle in handles {
        handle.recv_fast(Duration::from_secs(2))?;
        handle.recv_full(Duration::from_secs(2))?;
    }

    Ok(())
}

#[test]
fn test_async_pipeline_ttft_fast_state_under_50ms() -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig::default());
    let started = Instant::now();
    let handle = pipeline.submit_state(build_state("ttft"))?;
    handle.recv_fast(Duration::from_millis(500))?;
    let ttft = started.elapsed();

    assert!(
        ttft < Duration::from_millis(50),
        "TTFT regression: expected < 50ms, got {ttft:?}"
    );

    Ok(())
}
