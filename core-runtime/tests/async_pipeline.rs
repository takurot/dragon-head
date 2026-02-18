use std::{
    env, thread,
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

fn env_usize_with_default(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

fn env_u64_with_default(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

fn percentile_ms(samples: &[Duration], quantile: f64) -> f64 {
    debug_assert!(!samples.is_empty());
    let mut micros = samples
        .iter()
        .map(Duration::as_micros)
        .collect::<Vec<u128>>();
    micros.sort_unstable();

    let rank = ((micros.len().saturating_sub(1)) as f64 * quantile).ceil() as usize;
    micros[rank] as f64 / 1_000.0
}

fn average_ms(samples: &[Duration]) -> f64 {
    let total_micros: u128 = samples.iter().map(Duration::as_micros).sum();
    total_micros as f64 / samples.len() as f64 / 1_000.0
}

fn max_ms(samples: &[Duration]) -> f64 {
    samples.iter().map(Duration::as_micros).max().unwrap_or(0) as f64 / 1_000.0
}

fn run_ttft_benchmark(
    name: &str,
    iterations: usize,
    warn_ms: u64,
    fail_ms: u64,
) -> anyhow::Result<()> {
    let pipeline = AsyncPipeline::new(AsyncPipelineConfig::default());
    let mut samples = Vec::with_capacity(iterations);

    for idx in 0..iterations {
        let started = Instant::now();
        let handle = loop {
            match pipeline.submit_state(build_state(&format!("{name}-{idx}"))) {
                Ok(handle) => break handle,
                Err(_) => thread::sleep(Duration::from_millis(1)),
            }
        };

        handle.recv_fast(Duration::from_millis(500))?;
        samples.push(started.elapsed());
        handle.recv_full(Duration::from_millis(500))?;
    }

    let avg_ms = average_ms(&samples);
    let p95_ms = percentile_ms(&samples, 0.95);
    let p99_ms = percentile_ms(&samples, 0.99);
    let max_ms = max_ms(&samples);

    println!(
        "TTFT benchmark ({name}) samples={} avg={avg_ms:.3}ms p95={p95_ms:.3}ms p99={p99_ms:.3}ms max={max_ms:.3}ms warn={warn_ms}ms fail={fail_ms}ms",
        samples.len()
    );

    if p95_ms > warn_ms as f64 {
        println!(
            "::warning title=TTFT warning::p95 TTFT {p95_ms:.3}ms exceeded warning threshold {warn_ms}ms in {name} benchmark"
        );
    }

    assert!(
        p95_ms <= fail_ms as f64,
        "TTFT regression in {name} benchmark: p95={p95_ms:.3}ms exceeded fail threshold {fail_ms}ms"
    );

    Ok(())
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

#[test]
fn test_async_pipeline_ttft_benchmark_short_gate() -> anyhow::Result<()> {
    run_ttft_benchmark(
        "short",
        env_usize_with_default("TTFT_SHORT_ITERATIONS", 40),
        env_u64_with_default("TTFT_WARN_MS", 35),
        env_u64_with_default("TTFT_FAIL_MS", 50),
    )
}

#[test]
#[ignore = "Nightly long benchmark; run explicitly in scheduled CI"]
fn test_async_pipeline_ttft_benchmark_long_gate() -> anyhow::Result<()> {
    run_ttft_benchmark(
        "long",
        env_usize_with_default("TTFT_LONG_ITERATIONS", 400),
        env_u64_with_default("TTFT_WARN_MS", 40),
        env_u64_with_default("TTFT_FAIL_MS", 50),
    )
}
