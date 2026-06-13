use crate::config;
use core_runtime::{chrome_available, PolicyEngine};
use std::path::Path;

#[derive(Debug)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
    /// false means the issue is fatal; true with informational=true means non-fatal
    pub informational: bool,
    pub detail: String,
}

#[derive(Debug)]
pub struct DoctorReport {
    pub checks: Vec<CheckResult>,
}

impl DoctorReport {
    pub fn all_passed(&self) -> bool {
        self.checks.iter().all(|c| c.passed)
    }
}

fn chrome_path_detail_with_env(chrome_env: Option<&str>) -> (bool, String) {
    if let Some(path) = chrome_env {
        if Path::new(path).exists() {
            return (true, format!("CHROME_PATH={path}"));
        } else {
            return (false, format!("CHROME_PATH={path} (file not found)"));
        }
    }

    let candidates: &[&str] = if cfg!(target_os = "macos") {
        &[
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ]
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

    for candidate in candidates {
        if Path::new(candidate).exists() {
            return (true, candidate.to_string());
        }
    }

    if chrome_available() {
        return (true, "found via PATH".to_string());
    }

    (
        false,
        "not found — install Chrome/Chromium or set CHROME_PATH".to_string(),
    )
}

fn chrome_path_detail() -> (bool, String) {
    chrome_path_detail_with_env(std::env::var("CHROME_PATH").ok().as_deref())
}

/// Returns an error message if `resolved.chrome_path` or `resolved.policy_file` would fail
/// during server startup (see `main.rs`): a non-existent `chrome_path`, or a `policy.file` that
/// is missing or fails to parse via [`PolicyEngine::try_from_file`].
fn validate_resolved_paths(resolved: &config::ResolvedConfig) -> Result<(), String> {
    if let Some(chrome_path) = &resolved.chrome_path {
        if !Path::new(chrome_path).exists() {
            return Err(format!("chrome_path '{chrome_path}' does not exist"));
        }
    }

    if let Some(policy_file) = &resolved.policy_file {
        if let Err(err) = PolicyEngine::try_from_file(policy_file) {
            return Err(format!(
                "policy.file '{}' is invalid: {err}",
                policy_file.display()
            ));
        }
    }

    Ok(())
}

/// Resolves and validates the effective configuration via [`config::default_config_path_with`],
/// [`config::load_config_file`], [`config::resolve_config`], and [`validate_resolved_paths`].
///
/// `resolve_config` and `validate_resolved_paths` run even when no `config.toml` is present, so
/// environment-only misconfiguration (e.g. an invalid `PROMPT_INJECTION_MODE` or a nonexistent
/// `CHROME_PATH`) is caught here too — the same settings `main.rs` resolves at startup.
///
/// - No `$XDG_CONFIG_HOME`/`$HOME`, or `config.toml` not found -> informational (defaults apply)
///   as long as the resolved environment configuration is valid.
/// - `config.toml` present but unreadable/unparsable -> **fatal** (`passed: false`).
/// - Resolved configuration (file + env) is invalid, or `chrome_path`/`policy.file` would fail
///   at startup -> **fatal** (`passed: false`).
/// - Otherwise -> informational, detail includes a resolved-settings summary.
fn config_file_detail_with(lookup: impl Fn(&str) -> Option<String>) -> (bool, String, bool) {
    let path = config::default_config_path_with(&lookup);

    let file_config = match &path {
        Some(path) => match config::load_config_file(path) {
            Ok(file_config) => file_config,
            Err(err) => return (false, format!("{} — {err}", path.display()), false),
        },
        None => None,
    };

    let label = path
        .as_ref()
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "environment".to_string());

    let resolved = match config::resolve_config(file_config.as_ref(), &lookup) {
        Ok(resolved) => resolved,
        Err(err) => return (false, format!("{label} — {err}"), false),
    };

    if let Err(err) = validate_resolved_paths(&resolved) {
        return (false, format!("{label} — {err}"), false);
    }

    let summary = format!(
        "chrome_path={}, prompt_injection.mode={:?}, policy.file={}",
        resolved.chrome_path.as_deref().unwrap_or("<unset>"),
        resolved.injection_mode,
        resolved
            .policy_file
            .as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_else(|| "<unset>".to_string()),
    );

    let detail = match (&path, &file_config) {
        (Some(path), Some(_)) => format!("{} ({summary})", path.display()),
        (Some(path), None) => {
            format!(
                "{} (not found — defaults will be used; {summary})",
                path.display()
            )
        }
        (None, _) => format!("home dir not detected — defaults will be used; {summary}"),
    };

    (true, detail, true)
}

fn config_file_detail() -> (bool, String, bool) {
    config_file_detail_with(|key| std::env::var(key).ok())
}

pub fn run_doctor() -> DoctorReport {
    let (chrome_ok, chrome_detail) = chrome_path_detail();
    let (config_ok, config_detail, config_info) = config_file_detail();

    DoctorReport {
        checks: vec![
            CheckResult {
                name: "Chrome/Chromium",
                passed: chrome_ok,
                informational: false,
                detail: chrome_detail,
            },
            CheckResult {
                name: "Config file",
                passed: config_ok,
                informational: config_info,
                detail: config_detail,
            },
        ],
    }
}

pub fn print_report(report: &DoctorReport) {
    println!("dragon-head-mcp doctor");
    for check in &report.checks {
        let icon = if !check.passed {
            "✗"
        } else if check.informational {
            "ℹ"
        } else {
            "✓"
        };
        println!("  {icon} {}: {}", check.name, check.detail);
    }
    if report.all_passed() {
        println!("\nAll checks passed.");
    } else {
        println!("\nSome checks failed. See details above.");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn doctor_report_has_chrome_and_config_checks() {
        let report = run_doctor();
        assert_eq!(report.checks.len(), 2, "should have exactly 2 checks");
        assert_eq!(report.checks[0].name, "Chrome/Chromium");
        assert_eq!(report.checks[1].name, "Config file");
    }

    #[test]
    fn config_check_always_passes() {
        // Config absence is non-fatal — defaults apply.
        let report = run_doctor();
        assert!(report.checks[1].passed, "config check should always pass");
    }

    #[test]
    fn all_passed_reflects_checks() {
        let report = DoctorReport {
            checks: vec![
                CheckResult {
                    name: "a",
                    passed: true,
                    informational: false,
                    detail: "ok".into(),
                },
                CheckResult {
                    name: "b",
                    passed: false,
                    informational: false,
                    detail: "fail".into(),
                },
            ],
        };
        assert!(!report.all_passed());

        let all_ok = DoctorReport {
            checks: vec![CheckResult {
                name: "a",
                passed: true,
                informational: false,
                detail: "ok".into(),
            }],
        };
        assert!(all_ok.all_passed());
    }

    #[test]
    fn chrome_check_with_nonexistent_path_fails() {
        // Pass env directly — avoids mutating global env in a parallel test runner.
        let (passed, detail) = chrome_path_detail_with_env(Some("/nonexistent/path/to/chrome"));
        assert!(!passed);
        assert!(detail.contains("file not found"), "detail: {detail}");
    }

    #[test]
    fn chrome_check_with_no_env_falls_through_to_candidates() {
        // Without an env override, detection falls through to candidate paths and PATH.
        // The result depends on whether Chrome is installed; we only check the type.
        let (passed, detail) = chrome_path_detail_with_env(None);
        // detail must be non-empty regardless
        assert!(!detail.is_empty());
        // If not passed, the detail must explain why
        if !passed {
            assert!(
                detail.contains("not found"),
                "expected 'not found' message: {detail}"
            );
        }
    }

    #[test]
    fn config_check_fatal_for_malformed_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "this is not [valid toml",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(!passed, "malformed config.toml must fail the check");
        assert!(
            !informational,
            "a parse failure is fatal, not informational"
        );
        assert!(detail.contains("config.toml"), "detail: {detail}");
    }

    #[test]
    fn config_check_fatal_for_invalid_injection_mode() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "[prompt_injection]\nmode = \"verbose\"",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, _detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(!passed, "invalid prompt_injection.mode must fail the check");
        assert!(!informational);
    }

    #[test]
    fn config_check_informational_with_resolved_summary_for_valid_config() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "[prompt_injection]\nmode = \"redact\"",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(passed);
        assert!(informational);
        assert!(
            detail.contains("prompt_injection.mode=Redact"),
            "detail: {detail}"
        );
    }

    #[test]
    fn config_check_fatal_for_invalid_injection_mode_env_without_config_file() {
        let dir = tempfile::tempdir().unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        // No dragon-head/config.toml exists under `xdg`, but the env-only setting is invalid.
        let (passed, _detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            "PROMPT_INJECTION_MODE" => Some("verbose".to_string()),
            _ => None,
        });

        assert!(
            !passed,
            "invalid PROMPT_INJECTION_MODE must fail even without config.toml"
        );
        assert!(!informational);
    }

    #[test]
    fn config_check_fatal_for_nonexistent_chrome_path() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "chrome_path = \"/nonexistent/path/to/chrome\"",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(!passed, "nonexistent chrome_path must fail the check");
        assert!(!informational);
        assert!(detail.contains("chrome_path"), "detail: {detail}");
    }

    #[test]
    fn config_check_fatal_for_invalid_policy_file() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "[policy]\nfile = \"/nonexistent/policy.json\"",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(!passed, "invalid policy.file must fail the check");
        assert!(!informational);
        assert!(detail.contains("policy.file"), "detail: {detail}");
    }

    #[test]
    fn config_check_fatal_for_invalid_audit_durability() {
        let dir = tempfile::tempdir().unwrap();
        let dragon_head_dir = dir.path().join("dragon-head");
        std::fs::create_dir_all(&dragon_head_dir).unwrap();
        std::fs::write(
            dragon_head_dir.join("config.toml"),
            "[audit]\ndurability = \"eventual\"",
        )
        .unwrap();
        let xdg = dir.path().to_string_lossy().to_string();

        let (passed, _detail, informational) = config_file_detail_with(|key| match key {
            "XDG_CONFIG_HOME" => Some(xdg.clone()),
            _ => None,
        });

        assert!(!passed, "invalid audit.durability must fail the check");
        assert!(!informational);
    }

    #[test]
    fn config_check_informational_when_file_missing() {
        let (passed, _detail, informational) = config_file_detail();
        assert!(passed, "config check must always pass");
        // When the config file doesn't exist, it should be informational.
        // (On machines where the file exists, this assertion is skipped.)
        let config_base = std::env::var("XDG_CONFIG_HOME")
            .ok()
            .filter(|s| !s.is_empty())
            .unwrap_or_else(|| {
                std::env::var("HOME")
                    .map(|h| format!("{h}/.config"))
                    .unwrap_or_default()
            });
        let config_path = std::path::PathBuf::from(&config_base)
            .join("dragon-head")
            .join("config.toml");
        if !config_path.exists() {
            assert!(informational, "missing config file should be informational");
        }
    }
}
