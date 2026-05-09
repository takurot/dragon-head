use core_runtime::chrome_available;
use std::path::Path;

#[derive(Debug)]
pub struct CheckResult {
    pub name: &'static str,
    pub passed: bool,
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

fn chrome_path_detail() -> (bool, String) {
    if let Ok(path) = std::env::var("CHROME_PATH") {
        if Path::new(&path).exists() {
            return (true, format!("CHROME_PATH={path}"));
        } else {
            return (false, format!("CHROME_PATH={path} (file not found)"));
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

fn config_file_detail() -> (bool, String) {
    let config_path = std::env::var("HOME").ok().map(|h| {
        std::path::PathBuf::from(h)
            .join(".config")
            .join("dragon-head")
            .join("config.toml")
    });

    match config_path {
        Some(p) if p.exists() => (true, p.display().to_string()),
        Some(p) => (
            true,
            format!("{} (not found — defaults will be used)", p.display()),
        ),
        None => (
            true,
            "home dir not detected — defaults will be used".to_string(),
        ),
    }
}

pub fn run_doctor() -> DoctorReport {
    let (chrome_ok, chrome_detail) = chrome_path_detail();
    let (config_ok, config_detail) = config_file_detail();

    DoctorReport {
        checks: vec![
            CheckResult {
                name: "Chrome/Chromium",
                passed: chrome_ok,
                detail: chrome_detail,
            },
            CheckResult {
                name: "Config file",
                passed: config_ok,
                detail: config_detail,
            },
        ],
    }
}

pub fn print_report(report: &DoctorReport) {
    println!("dragon-head-mcp doctor");
    for check in &report.checks {
        let icon = if check.passed { "✓" } else { "✗" };
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
                    detail: "ok".into(),
                },
                CheckResult {
                    name: "b",
                    passed: false,
                    detail: "fail".into(),
                },
            ],
        };
        assert!(!report.all_passed());

        let all_ok = DoctorReport {
            checks: vec![CheckResult {
                name: "a",
                passed: true,
                detail: "ok".into(),
            }],
        };
        assert!(all_ok.all_passed());
    }

    #[test]
    fn chrome_check_reflects_env_var() {
        // When CHROME_PATH points to a non-existent file, Chrome check fails.
        let original = std::env::var("CHROME_PATH").ok();
        std::env::set_var("CHROME_PATH", "/nonexistent/path/to/chrome");
        let (passed, detail) = chrome_path_detail();
        // Restore before asserting so any failure doesn't leave env dirty.
        if let Some(v) = original {
            std::env::set_var("CHROME_PATH", v);
        } else {
            std::env::remove_var("CHROME_PATH");
        }
        assert!(!passed);
        assert!(detail.contains("file not found"), "detail: {detail}");
    }
}
