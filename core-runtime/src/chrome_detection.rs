use std::path::Path;
use std::process::Command;

pub fn chrome_available() -> bool {
    if let Ok(path) = std::env::var("CHROME_PATH") {
        if Path::new(&path).exists() {
            return true;
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
        return true;
    }

    let command_names = &[
        "chromium",
        "chromium-browser",
        "google-chrome",
        "google-chrome-stable",
    ];
    command_names.iter().any(|name| {
        Command::new("which")
            .arg(name)
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false)
    })
}

pub fn should_skip_browser_tests() -> bool {
    !chrome_available()
}
