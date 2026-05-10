use core_runtime::chrome_available;
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

fn config_file_detail() -> (bool, String, bool) {
    // Respect XDG_CONFIG_HOME on Linux; fall back to $HOME/.config.
    let config_base = std::env::var("XDG_CONFIG_HOME")
        .ok()
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| {
            std::env::var("HOME")
                .map(|h| format!("{h}/.config"))
                .unwrap_or_default()
        });

    if config_base.is_empty() {
        return (
            true,
            "home dir not detected — defaults will be used".to_string(),
            true,
        );
    }

    let p = std::path::PathBuf::from(&config_base)
        .join("dragon-head")
        .join("config.toml");

    if p.exists() {
        (true, p.display().to_string(), false)
    } else {
        (
            true,
            format!("{} (not found — defaults will be used)", p.display()),
            true,
        )
    }
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
