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
        match env::var("DRAGON_HEAD_EVAL_MODE")
            .unwrap_or_else(|_| "smoke".to_string())
            .to_ascii_lowercase()
            .as_str()
        {
            "full" => Self::Full,
            _ => Self::Smoke,
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
        F: FnOnce() -> Result<Value>,
    {
        let started = Instant::now();
        match scenario() {
            Ok(details) => self.push_record(ScenarioRecord {
                scenario_id: scenario_id.to_string(),
                feature_area: feature_area.to_string(),
                status: ScenarioStatus::Passed,
                duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
                details,
                error: None,
            }),
            Err(err) => self.push_record(ScenarioRecord {
                scenario_id: scenario_id.to_string(),
                feature_area: feature_area.to_string(),
                status: ScenarioStatus::Failed,
                duration_ms: started.elapsed().as_secs_f64() * 1_000.0,
                details: Value::Null,
                error: Some(format!("{err:#}")),
            }),
        }
    }

    pub fn write_if_configured(&self) -> Result<Option<PathBuf>> {
        let Ok(output_dir) = env::var("DRAGON_HEAD_EVAL_OUTPUT_DIR") else {
            return Ok(None);
        };

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

fn sanitize_component(input: &str) -> String {
    input
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect()
}
