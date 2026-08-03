use std::{
    env, fs,
    path::{Path, PathBuf},
    process::Command,
    time::Instant,
};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Returns `true` when no Chrome/Chromium binary is available, indicating that
/// browser-dependent tests should be skipped.
///
/// Usage in a test:
/// ```ignore
/// if test_bench_support::should_skip_browser_tests() { return Ok(()); }
/// ```
#[must_use]
pub fn should_skip_browser_tests() -> bool {
    if let Ok(path) = env::var("CHROME_PATH") {
        if Path::new(&path).exists() {
            return false;
        }
    }

    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &["/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"]
    } else if cfg!(target_os = "linux") {
        &[
            "/usr/bin/chromium",
            "/usr/bin/chromium-browser",
            "/usr/bin/google-chrome",
            "/usr/bin/google-chrome-stable",
        ]
    } else if cfg!(target_os = "windows") {
        &[
            "C:\\Program Files\\Google\\Chrome\\Application\\chrome.exe",
            "C:\\Program Files (x86)\\Google\\Chrome\\Application\\chrome.exe",
        ]
    } else {
        &[]
    };

    if candidates.iter().any(|p| Path::new(p).exists()) {
        return false;
    }

    #[cfg(not(target_os = "windows"))]
    let which_cmd = "which";
    #[cfg(target_os = "windows")]
    let which_cmd = "where";

    let found_via_which = [
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ]
    .iter()
    .any(|name| {
        Command::new(which_cmd)
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    });

    !found_via_which
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationMode {
    Smoke,
    Full,
}

impl EvaluationMode {
    pub fn from_env() -> Self {
        Self::from_env_value(env::var("DRAGON_HEAD_EVAL_MODE").ok().as_deref())
    }

    fn from_env_value(raw: Option<&str>) -> Self {
        match raw {
            None => Self::Smoke,
            Some(value) => match value.to_ascii_lowercase().as_str() {
                "smoke" => Self::Smoke,
                "full" => Self::Full,
                other => panic!(
                    "invalid DRAGON_HEAD_EVAL_MODE={other:?}: expected \"smoke\" or \"full\""
                ),
            },
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Smoke => "smoke",
            Self::Full => "full",
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ScenarioStatus {
    Passed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ScenarioRecord {
    pub scenario_id: String,
    pub feature_area: String,
    pub status: ScenarioStatus,
    pub duration_ms: f64,
    pub details: Value,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, Default)]
pub struct EvaluationSummary {
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct EvaluationReport {
    pub crate_name: String,
    pub suite_name: String,
    pub mode: EvaluationMode,
    pub scenarios: Vec<ScenarioRecord>,
    pub summary: EvaluationSummary,
}

pub struct EvaluationBench {
    report: EvaluationReport,
}

impl EvaluationBench {
    pub fn new(crate_name: &str, suite_name: &str, mode: EvaluationMode) -> Self {
        Self {
            report: EvaluationReport {
                crate_name: crate_name.to_string(),
                suite_name: suite_name.to_string(),
                mode,
                scenarios: Vec::new(),
                summary: EvaluationSummary::default(),
            },
        }
    }

    pub fn run_scenario<F>(&mut self, scenario_id: &str, feature_area: &str, scenario: F)
    where
        F: FnOnce() -> Result<Value> + std::panic::UnwindSafe,
    {
        let started = Instant::now();
        let outcome = std::panic::catch_unwind(scenario);
        let duration_ms = started.elapsed().as_secs_f64() * 1_000.0;
        match outcome {
            Ok(Ok(details)) => self.push_record(ScenarioRecord {
                scenario_id: scenario_id.to_string(),
                feature_area: feature_area.to_string(),
                status: ScenarioStatus::Passed,
                duration_ms,
                details,
                error: None,
            }),
            Ok(Err(err)) => self.push_record(ScenarioRecord {
                scenario_id: scenario_id.to_string(),
                feature_area: feature_area.to_string(),
                status: ScenarioStatus::Failed,
                duration_ms,
                details: Value::Null,
                error: Some(format!("{err:#}")),
            }),
            Err(panic) => self.push_record(ScenarioRecord {
                scenario_id: scenario_id.to_string(),
                feature_area: feature_area.to_string(),
                status: ScenarioStatus::Failed,
                duration_ms,
                details: Value::Null,
                error: Some(format!("scenario panicked: {}", panic_message(&*panic))),
            }),
        }
    }

    pub fn write_if_configured(&self) -> Result<Option<PathBuf>> {
        let Ok(output_dir) = env::var("DRAGON_HEAD_EVAL_OUTPUT_DIR") else {
            return Ok(None);
        };
        validate_output_dir(&output_dir)?;

        let file_name = format!(
            "{}--{}.json",
            sanitize_component(&self.report.crate_name),
            sanitize_component(&self.report.suite_name)
        );
        let path = Path::new(&output_dir).join(file_name);

        fs::create_dir_all(&output_dir).with_context(|| {
            format!(
                "failed to create evaluation output directory at {}",
                Path::new(&output_dir).display()
            )
        })?;
        let body = serde_json::to_vec_pretty(&self.report)
            .context("failed to serialize evaluation report")?;
        fs::write(&path, body)
            .with_context(|| format!("failed to write evaluation report at {}", path.display()))?;

        Ok(Some(path))
    }

    pub fn assert_required_scenarios(&self, required: &[&str]) -> Result<()> {
        let missing = required
            .iter()
            .filter(|scenario_id| {
                !self
                    .report
                    .scenarios
                    .iter()
                    .any(|record| record.scenario_id == **scenario_id)
            })
            .copied()
            .collect::<Vec<_>>();

        if missing.is_empty() {
            return Ok(());
        }

        anyhow::bail!(
            "missing required evaluation scenarios: {}",
            missing.join(", ")
        );
    }

    pub fn assert_all_passed(&self) -> Result<()> {
        let failed = self
            .report
            .scenarios
            .iter()
            .filter(|record| record.status == ScenarioStatus::Failed)
            .map(|record| match &record.error {
                Some(error) => format!("{} ({error})", record.scenario_id),
                None => record.scenario_id.clone(),
            })
            .collect::<Vec<_>>();

        if failed.is_empty() {
            return Ok(());
        }

        anyhow::bail!("evaluation scenarios failed: {}", failed.join("; "));
    }

    pub fn report(&self) -> &EvaluationReport {
        &self.report
    }

    fn push_record(&mut self, record: ScenarioRecord) {
        self.report.summary.total += 1;
        match record.status {
            ScenarioStatus::Passed => self.report.summary.passed += 1,
            ScenarioStatus::Failed => self.report.summary.failed += 1,
        }
        self.report.scenarios.push(record);
    }
}

/// Rejects `DRAGON_HEAD_EVAL_OUTPUT_DIR` values containing `..` path
/// components, which could otherwise be used to write the evaluation report
/// outside the intended output directory.
fn validate_output_dir(output_dir: &str) -> Result<()> {
    let has_parent_dir = Path::new(output_dir)
        .components()
        .any(|c| matches!(c, std::path::Component::ParentDir));
    if has_parent_dir {
        anyhow::bail!("DRAGON_HEAD_EVAL_OUTPUT_DIR must not contain '..' components: {output_dir}");
    }
    Ok(())
}

/// Extracts a human-readable message from a `catch_unwind` payload.
fn panic_message(panic: &(dyn std::any::Any + Send)) -> String {
    if let Some(s) = panic.downcast_ref::<&str>() {
        (*s).to_string()
    } else if let Some(s) = panic.downcast_ref::<String>() {
        s.clone()
    } else {
        "unknown panic payload".to_string()
    }
}

/// Sanitizes a component for use in a filename. Distinct inputs are mapped to
/// distinct outputs: allowed characters pass through unchanged, everything
/// else is escaped as `_u{hex codepoint}_`. `_` itself is always escaped
/// (never passed through) since it is the escape delimiter — otherwise a
/// literal `_u2e_` in the input would collide with the escaped form of `.`.
fn sanitize_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii_alphanumeric() || ch == '-' {
            out.push(ch);
        } else {
            out.push_str(&format!("_u{:x}_", ch as u32));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn run_scenario_records_failure_when_scenario_panics() {
        let mut bench = EvaluationBench::new("crate", "suite", EvaluationMode::Smoke);

        bench.run_scenario("panicking", "area", || panic!("boom"));

        assert_eq!(bench.report().summary.total, 1);
        assert_eq!(bench.report().summary.passed, 0);
        assert_eq!(bench.report().summary.failed, 1);
        let record = &bench.report().scenarios[0];
        assert_eq!(record.status, ScenarioStatus::Failed);
        assert_eq!(record.error.as_deref(), Some("scenario panicked: boom"));
    }

    #[test]
    fn run_scenario_still_records_subsequent_scenarios_after_a_panic() {
        let mut bench = EvaluationBench::new("crate", "suite", EvaluationMode::Smoke);

        bench.run_scenario("panicking", "area", || panic!("boom"));
        bench.run_scenario("ok", "area", || Ok(Value::Null));

        assert_eq!(bench.report().summary.total, 2);
        assert_eq!(bench.report().summary.passed, 1);
        assert_eq!(bench.report().summary.failed, 1);
    }

    #[test]
    fn from_env_value_defaults_to_smoke_when_unset() {
        assert_eq!(EvaluationMode::from_env_value(None), EvaluationMode::Smoke);
    }

    #[test]
    fn from_env_value_accepts_known_modes_case_insensitively() {
        assert_eq!(
            EvaluationMode::from_env_value(Some("FULL")),
            EvaluationMode::Full
        );
        assert_eq!(
            EvaluationMode::from_env_value(Some("smoke")),
            EvaluationMode::Smoke
        );
    }

    #[test]
    #[should_panic(expected = "invalid DRAGON_HEAD_EVAL_MODE")]
    fn from_env_value_panics_on_unrecognized_mode() {
        EvaluationMode::from_env_value(Some("bogus"));
    }

    #[test]
    fn sanitize_component_does_not_collide_distinct_inputs() {
        assert_ne!(sanitize_component("a.b"), sanitize_component("a_b"));
        assert_ne!(sanitize_component("a.b"), sanitize_component("a b"));
        assert_ne!(sanitize_component("a.b"), sanitize_component("a_u2e_b"));
    }

    #[test]
    fn validate_output_dir_rejects_parent_dir_traversal() {
        assert!(validate_output_dir("../escape").is_err());
        assert!(validate_output_dir("target/eval/../../escape").is_err());
    }

    #[test]
    fn validate_output_dir_accepts_plain_paths() {
        assert!(validate_output_dir("target/eval-output").is_ok());
        assert!(validate_output_dir("/tmp/eval-output").is_ok());
    }
}
