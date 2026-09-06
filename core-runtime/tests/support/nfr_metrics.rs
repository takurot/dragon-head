#![allow(dead_code)]

use std::{env, fs, time::Duration};

use anyhow::Context;
use serde_json::Value;

pub fn bench_mode() -> String {
    env::var("NFR_BENCH_MODE").unwrap_or_else(|_| "short".to_string())
}

pub fn env_usize_with_default(key: &str, default: usize) -> usize {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<usize>().ok())
        .filter(|value| *value > 0)
        .unwrap_or(default)
}

pub fn env_u64_with_default(key: &str, default: u64) -> u64 {
    env::var(key)
        .ok()
        .and_then(|value| value.parse::<u64>().ok())
        .unwrap_or(default)
}

pub fn percentile_duration_ms(samples: &[Duration], quantile: f64) -> f64 {
    // An empty sample set has no percentile to report — return 0.0 rather
    // than panicking (this guard must hold in release too, not just debug;
    // `debug_assert!` alone is stripped in release builds, at which point
    // `micros[rank]` on an empty vec would panic anyway, just with a less
    // useful message). See the identical fix on `percentile_f64` below
    // (issue #281).
    if samples.is_empty() {
        return 0.0;
    }
    let mut micros = samples
        .iter()
        .map(Duration::as_micros)
        .collect::<Vec<u128>>();
    micros.sort_unstable();

    let rank = ((micros.len().saturating_sub(1)) as f64 * quantile).ceil() as usize;
    micros[rank] as f64 / 1_000.0
}

pub fn average_duration_ms(samples: &[Duration]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let total_micros: u128 = samples.iter().map(Duration::as_micros).sum();
    total_micros as f64 / samples.len() as f64 / 1_000.0
}

pub fn max_duration_ms(samples: &[Duration]) -> f64 {
    samples.iter().map(Duration::as_micros).max().unwrap_or(0) as f64 / 1_000.0
}

pub fn percentile_f64(samples: &[f64], quantile: f64) -> f64 {
    // See `percentile_duration_ms` above — same empty-input fix (issue #281).
    if samples.is_empty() {
        return 0.0;
    }
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));

    let rank = ((sorted.len().saturating_sub(1)) as f64 * quantile).ceil() as usize;
    sorted[rank]
}

pub fn average_f64(samples: &[f64]) -> f64 {
    if samples.is_empty() {
        return 0.0;
    }
    let total: f64 = samples.iter().sum();
    total / samples.len() as f64
}

pub fn min_f64(samples: &[f64]) -> f64 {
    samples.iter().copied().fold(f64::INFINITY, f64::min)
}

pub fn write_metric(metric_id: &str, payload: Value) -> anyhow::Result<()> {
    let Ok(metrics_dir) = env::var("NFR_METRICS_DIR") else {
        return Ok(());
    };

    let safe_metric_id = metric_id
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect::<String>();
    let output_path = std::path::Path::new(&metrics_dir).join(format!("{safe_metric_id}.json"));

    fs::create_dir_all(&metrics_dir).with_context(|| {
        format!(
            "failed to create NFR metrics directory at {}",
            std::path::Path::new(&metrics_dir).display()
        )
    })?;

    let body = serde_json::to_vec_pretty(&payload)
        .with_context(|| format!("failed to serialize NFR metric payload for {metric_id}"))?;
    fs::write(&output_path, body).with_context(|| {
        format!(
            "failed to write NFR metrics file for {metric_id} at {}",
            output_path.display()
        )
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn average_duration_ms_returns_zero_for_empty_samples() {
        assert_eq!(average_duration_ms(&[]), 0.0);
    }

    #[test]
    fn average_f64_returns_zero_for_empty_samples() {
        assert_eq!(average_f64(&[]), 0.0);
    }

    #[test]
    fn percentile_duration_ms_returns_zero_for_empty_samples() {
        assert_eq!(percentile_duration_ms(&[], 0.95), 0.0);
    }

    #[test]
    fn percentile_f64_returns_zero_for_empty_samples() {
        assert_eq!(percentile_f64(&[], 0.95), 0.0);
    }

    #[test]
    fn percentile_duration_ms_still_computes_correctly_for_nonempty_samples() {
        let samples = [
            Duration::from_millis(10),
            Duration::from_millis(20),
            Duration::from_millis(30),
        ];
        assert_eq!(average_duration_ms(&samples), 20.0);
        assert_eq!(percentile_duration_ms(&samples, 1.0), 30.0);
    }
}
