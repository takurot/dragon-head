//! Loads `dragon-head-mcp`'s optional `config.toml` and resolves it together with
//! environment-variable overrides.
//!
//! See ISSUE-146: `--doctor` previously implied this file was consumed even though nothing
//! read it. All resolution logic here is pure (takes an injected `lookup` closure instead of
//! reading `std::env` directly) so it can be unit tested without `std::env::set_var`.

use core_runtime::PromptInjectionMode;
use serde::Deserialize;
use std::{
    collections::HashSet,
    path::{Path, PathBuf},
};

pub const ENV_CHROME_PATH: &str = "CHROME_PATH";
pub const ENV_PROMPT_INJECTION_MODE: &str = "PROMPT_INJECTION_MODE";
pub const ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES: &str = "PROMPT_INJECTION_ADDITIONAL_PHRASES";
pub const ENV_POLICY_FILE: &str = "POLICY_FILE";
pub const ENV_AUDIT_LOG_DIR: &str = "AUDIT_LOG_DIR";
pub const ENV_AUDIT_LOG_MAX_BYTES: &str = "AUDIT_LOG_MAX_BYTES";
pub const ENV_AUDIT_DURABILITY: &str = "AUDIT_DURABILITY";
/// Historical name retained for compatibility; audit events are mirrored to stderr.
pub const ENV_AUDIT_LOG_STDERR_MIRROR: &str = "AUDIT_LOG_STDOUT";

pub const HONORED_CONFIG_ENV_VARS: &[&str] = &[
    ENV_CHROME_PATH,
    ENV_PROMPT_INJECTION_MODE,
    ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
    ENV_POLICY_FILE,
    ENV_AUDIT_LOG_DIR,
    ENV_AUDIT_LOG_MAX_BYTES,
    ENV_AUDIT_DURABILITY,
    ENV_AUDIT_LOG_STDERR_MIRROR,
];

pub const MAX_ADDITIONAL_PHRASES: usize = 64;
pub const MAX_ADDITIONAL_PHRASE_BYTES: usize = 512;
pub const MAX_ADDITIONAL_PHRASES_BYTES: usize = 8 * 1024;

/// Raw `config.toml` contents.
#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct FileConfig {
    pub chrome_path: Option<String>,
    #[serde(default)]
    pub prompt_injection: PromptInjectionFileConfig,
    #[serde(default)]
    pub policy: PolicyFileConfig,
    #[serde(default)]
    pub audit: AuditFileConfig,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct PromptInjectionFileConfig {
    /// `"off"`, `"report_only"`, or `"redact"`.
    pub mode: Option<String>,
    /// Additional literal phrases matched by the prompt-injection sanitizer.
    #[serde(default)]
    pub additional_phrases: Vec<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct PolicyFileConfig {
    /// Path to a JSON file of `PolicyRule`s, loaded via `PolicyEngine::try_from_file`.
    pub file: Option<String>,
}

#[derive(Debug, Clone, Default, Deserialize, PartialEq)]
pub struct AuditFileConfig {
    /// Mirrors `AUDIT_LOG_DIR`.
    pub log_dir: Option<String>,
    /// Mirrors `AUDIT_LOG_MAX_BYTES`.
    pub max_bytes: Option<u64>,
    /// Mirrors `AUDIT_DURABILITY` (`"flush"` or `"sync"`).
    pub durability: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("failed to read config file {path}: {source}")]
    Io {
        path: PathBuf,
        #[source]
        source: std::io::Error,
    },
    #[error("failed to parse config file {path}")]
    Parse {
        path: PathBuf,
        details: toml::de::Error,
    },
    #[error("invalid prompt_injection.mode '{0}' (expected 'off', 'report_only', or 'redact')")]
    InvalidInjectionMode(String),
    #[error("invalid audit.durability '{0}' (expected 'flush' or 'sync')")]
    InvalidAuditDurability(String),
    #[error(
        "invalid {ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES} (expected a JSON array of strings)"
    )]
    InvalidAdditionalPhrasesFormat,
    #[error("too many prompt-injection additional phrases (maximum {max})")]
    TooManyAdditionalPhrases { max: usize },
    #[error("prompt-injection additional phrase exceeds {max_bytes} UTF-8 bytes")]
    AdditionalPhraseTooLong { max_bytes: usize },
    #[error("prompt-injection additional phrases exceed {max_bytes} total UTF-8 bytes")]
    AdditionalPhrasesTooLarge { max_bytes: usize },
    #[error("environment variable {name} is not valid UTF-8")]
    NonUnicodeEnvVar { name: &'static str },
}

pub fn validate_unicode_additional_phrases_env() -> Result<(), ConfigError> {
    match std::env::var(ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES) {
        Ok(_) | Err(std::env::VarError::NotPresent) => Ok(()),
        Err(std::env::VarError::NotUnicode(_)) => Err(ConfigError::NonUnicodeEnvVar {
            name: ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES,
        }),
    }
}

/// The default config file path: `$XDG_CONFIG_HOME/dragon-head/config.toml`, falling back to
/// `$HOME/.config/dragon-head/config.toml`. Returns `None` if neither variable is set.
pub fn default_config_path_with(lookup: impl Fn(&str) -> Option<String>) -> Option<PathBuf> {
    let base = lookup("XDG_CONFIG_HOME")
        .filter(|s| !s.is_empty())
        .or_else(|| {
            lookup("HOME")
                .filter(|s| !s.is_empty())
                .map(|home| format!("{home}/.config"))
        })?;

    Some(PathBuf::from(base).join("dragon-head").join("config.toml"))
}

/// Loads and parses `path`.
///
/// - `Ok(None)`: the file does not exist — callers should fall back to defaults.
/// - `Ok(Some(_))`: the file exists and parsed successfully (an empty file parses to
///   `FileConfig::default()`; unrecognized top-level keys are ignored).
/// - `Err(_)`: the file exists but could not be read or parsed — callers should treat this as
///   a fatal misconfiguration, not silently fall back.
pub fn load_config_file(path: &Path) -> Result<Option<FileConfig>, ConfigError> {
    let body = match std::fs::read_to_string(path) {
        Ok(body) => body,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => {
            return Err(ConfigError::Io {
                path: path.to_path_buf(),
                source: err,
            })
        }
    };

    toml::from_str(&body)
        .map(Some)
        .map_err(|err| ConfigError::Parse {
            path: path.to_path_buf(),
            details: err,
        })
}

/// Effective configuration after merging `file_config` with environment-variable overrides.
/// Environment variables always win.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ResolvedConfig {
    pub chrome_path: Option<String>,
    pub injection_mode: PromptInjectionMode,
    pub injection_additional_phrases: Vec<String>,
    pub policy_file: Option<PathBuf>,
    pub audit_log_dir: Option<String>,
    pub audit_max_bytes: Option<u64>,
    pub audit_durability: Option<String>,
}

/// Merges `file_config` (`None` means "no config file present, use defaults") with
/// environment-variable overrides supplied via `lookup`.
///
/// Precedence (env wins): `CHROME_PATH` > `chrome_path`; `PROMPT_INJECTION_MODE` >
/// `prompt_injection.mode` (default `report_only`);
/// `PROMPT_INJECTION_ADDITIONAL_PHRASES` > `prompt_injection.additional_phrases`;
/// `POLICY_FILE` > `policy.file`;
/// `AUDIT_LOG_DIR`/`AUDIT_LOG_MAX_BYTES`/`AUDIT_DURABILITY` > `audit.*`.
pub fn resolve_config(
    file_config: Option<&FileConfig>,
    lookup: impl Fn(&str) -> Option<String>,
) -> Result<ResolvedConfig, ConfigError> {
    let empty = FileConfig::default();
    let fc = file_config.unwrap_or(&empty);

    let chrome_path = lookup(ENV_CHROME_PATH).or_else(|| fc.chrome_path.clone());

    let injection_mode =
        match lookup(ENV_PROMPT_INJECTION_MODE).or_else(|| fc.prompt_injection.mode.clone()) {
            None => PromptInjectionMode::ReportOnly,
            Some(mode) => parse_injection_mode(&mode)?,
        };

    let injection_additional_phrases = match lookup(ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES) {
        Some(raw) => serde_json::from_str::<Vec<String>>(&raw)
            .map_err(|_| ConfigError::InvalidAdditionalPhrasesFormat)?,
        None => fc.prompt_injection.additional_phrases.clone(),
    };
    let injection_additional_phrases = normalize_additional_phrases(injection_additional_phrases)?;

    let policy_file = lookup(ENV_POLICY_FILE)
        .or_else(|| fc.policy.file.clone())
        .map(PathBuf::from);

    let audit_log_dir = lookup(ENV_AUDIT_LOG_DIR).or_else(|| fc.audit.log_dir.clone());

    let audit_max_bytes = match lookup(ENV_AUDIT_LOG_MAX_BYTES) {
        Some(raw) => match raw.parse::<u64>() {
            Ok(bytes) => Some(bytes),
            Err(_) => {
                eprintln!(
                    "[CONFIG][WARN] AUDIT_LOG_MAX_BYTES='{raw}' is not a valid integer; \
                     falling back to config.toml audit.max_bytes (or default)."
                );
                fc.audit.max_bytes
            }
        },
        None => fc.audit.max_bytes,
    };

    let audit_durability = lookup(ENV_AUDIT_DURABILITY).or_else(|| fc.audit.durability.clone());
    if let Some(durability) = &audit_durability {
        if durability != "flush" && durability != "sync" {
            return Err(ConfigError::InvalidAuditDurability(durability.clone()));
        }
    }

    Ok(ResolvedConfig {
        chrome_path,
        injection_mode,
        injection_additional_phrases,
        policy_file,
        audit_log_dir,
        audit_max_bytes,
        audit_durability,
    })
}

fn normalize_additional_phrases(phrases: Vec<String>) -> Result<Vec<String>, ConfigError> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    let mut total_bytes = 0usize;
    for phrase in phrases {
        let phrase = phrase.trim();
        if phrase.is_empty() {
            continue;
        }
        let phrase = phrase.to_string();
        if !seen.insert(phrase.clone()) {
            continue;
        }
        if normalized.len() == MAX_ADDITIONAL_PHRASES {
            return Err(ConfigError::TooManyAdditionalPhrases {
                max: MAX_ADDITIONAL_PHRASES,
            });
        }
        if phrase.len() > MAX_ADDITIONAL_PHRASE_BYTES {
            return Err(ConfigError::AdditionalPhraseTooLong {
                max_bytes: MAX_ADDITIONAL_PHRASE_BYTES,
            });
        }
        total_bytes += phrase.len();
        if total_bytes > MAX_ADDITIONAL_PHRASES_BYTES {
            return Err(ConfigError::AdditionalPhrasesTooLarge {
                max_bytes: MAX_ADDITIONAL_PHRASES_BYTES,
            });
        }
        normalized.push(phrase);
    }
    Ok(normalized)
}

fn parse_injection_mode(mode: &str) -> Result<PromptInjectionMode, ConfigError> {
    match mode {
        "off" => Ok(PromptInjectionMode::Off),
        "report_only" => Ok(PromptInjectionMode::ReportOnly),
        "redact" => Ok(PromptInjectionMode::Redact),
        other => Err(ConfigError::InvalidInjectionMode(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn no_env(_: &str) -> Option<String> {
        None
    }

    fn write_config(dir: &tempfile::TempDir, contents: &str) -> PathBuf {
        let path = dir.path().join("config.toml");
        std::fs::write(&path, contents).unwrap();
        path
    }

    // --- default_config_path_with ---

    #[test]
    fn default_path_uses_xdg_config_home_when_set() {
        let path = default_config_path_with(|key| match key {
            "XDG_CONFIG_HOME" => Some("/xdg".to_string()),
            "HOME" => Some("/home/user".to_string()),
            _ => None,
        });
        assert_eq!(path, Some(PathBuf::from("/xdg/dragon-head/config.toml")));
    }

    #[test]
    fn default_path_falls_back_to_home_config_when_xdg_unset() {
        let path = default_config_path_with(|key| match key {
            "HOME" => Some("/home/user".to_string()),
            _ => None,
        });
        assert_eq!(
            path,
            Some(PathBuf::from("/home/user/.config/dragon-head/config.toml"))
        );
    }

    #[test]
    fn default_path_is_none_when_neither_var_set() {
        assert_eq!(default_config_path_with(no_env), None);
    }

    // --- load_config_file ---

    #[test]
    fn load_config_file_returns_none_for_missing_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("does-not-exist.toml");
        assert_eq!(load_config_file(&path).unwrap(), None);
    }

    #[test]
    fn load_config_file_returns_parse_error_for_malformed_toml() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "this is not [valid toml");
        let err = load_config_file(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "got: {err:?}");
    }

    #[test]
    fn load_config_file_parses_full_example() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            r#"
chrome_path = "/usr/bin/chromium"

[prompt_injection]
mode = "redact"
additional_phrases = ["reveal developer message"]

[policy]
file = "/etc/dragon-head/policy.json"

[audit]
log_dir = "/var/log/dragon-head"
max_bytes = 1048576
durability = "sync"
"#,
        );
        let config = load_config_file(&path).unwrap().unwrap();
        assert_eq!(
            config,
            FileConfig {
                chrome_path: Some("/usr/bin/chromium".to_string()),
                prompt_injection: PromptInjectionFileConfig {
                    mode: Some("redact".to_string()),
                    additional_phrases: vec!["reveal developer message".to_string()],
                },
                policy: PolicyFileConfig {
                    file: Some("/etc/dragon-head/policy.json".to_string()),
                },
                audit: AuditFileConfig {
                    log_dir: Some("/var/log/dragon-head".to_string()),
                    max_bytes: Some(1_048_576),
                    durability: Some("sync".to_string()),
                },
            }
        );
    }

    #[test]
    fn load_config_file_applies_defaults_for_missing_tables() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, r#"chrome_path = "/usr/bin/chromium""#);
        let config = load_config_file(&path).unwrap().unwrap();
        assert_eq!(config.chrome_path, Some("/usr/bin/chromium".to_string()));
        assert_eq!(
            config.prompt_injection,
            PromptInjectionFileConfig::default()
        );
        assert_eq!(config.policy, PolicyFileConfig::default());
        assert_eq!(config.audit, AuditFileConfig::default());
    }

    #[test]
    fn load_config_file_empty_file_is_all_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "");
        assert_eq!(
            load_config_file(&path).unwrap(),
            Some(FileConfig::default())
        );
    }

    #[test]
    fn load_config_file_ignores_unknown_top_level_keys() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(
            &dir,
            "unknown_key = \"value\"\nchrome_path = \"/usr/bin/chromium\"",
        );
        let config = load_config_file(&path).unwrap().unwrap();
        assert_eq!(config.chrome_path, Some("/usr/bin/chromium".to_string()));
    }

    #[test]
    fn load_config_file_rejects_non_numeric_audit_max_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_config(&dir, "[audit]\nmax_bytes = \"not-a-number\"");
        let err = load_config_file(&path).unwrap_err();
        assert!(matches!(err, ConfigError::Parse { .. }), "got: {err:?}");
    }

    // --- resolve_config ---

    #[test]
    fn resolve_config_defaults_when_no_file_and_no_env() {
        let resolved = resolve_config(None, no_env).unwrap();
        assert_eq!(
            resolved,
            ResolvedConfig {
                chrome_path: None,
                injection_mode: PromptInjectionMode::ReportOnly,
                injection_additional_phrases: vec![],
                policy_file: None,
                audit_log_dir: None,
                audit_max_bytes: None,
                audit_durability: None,
            }
        );
    }

    #[test]
    fn resolve_config_uses_file_values_when_no_env_override() {
        let fc = FileConfig {
            chrome_path: Some("/usr/bin/chromium".to_string()),
            prompt_injection: PromptInjectionFileConfig {
                mode: Some("redact".to_string()),
                additional_phrases: vec!["reveal developer message".to_string()],
            },
            policy: PolicyFileConfig {
                file: Some("/etc/policy.json".to_string()),
            },
            audit: AuditFileConfig {
                log_dir: Some("/var/log/dh".to_string()),
                max_bytes: Some(2048),
                durability: Some("sync".to_string()),
            },
        };
        let resolved = resolve_config(Some(&fc), no_env).unwrap();
        assert_eq!(resolved.chrome_path, Some("/usr/bin/chromium".to_string()));
        assert_eq!(resolved.injection_mode, PromptInjectionMode::Redact);
        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["reveal developer message".to_string()]
        );
        assert_eq!(
            resolved.policy_file,
            Some(PathBuf::from("/etc/policy.json"))
        );
        assert_eq!(resolved.audit_log_dir, Some("/var/log/dh".to_string()));
        assert_eq!(resolved.audit_max_bytes, Some(2048));
        assert_eq!(resolved.audit_durability, Some("sync".to_string()));
    }

    #[test]
    fn resolve_config_env_vars_override_file_values() {
        let fc = FileConfig {
            chrome_path: Some("/usr/bin/chromium".to_string()),
            prompt_injection: PromptInjectionFileConfig {
                mode: Some("redact".to_string()),
                additional_phrases: vec!["reveal developer message".to_string()],
            },
            policy: PolicyFileConfig {
                file: Some("/etc/policy.json".to_string()),
            },
            audit: AuditFileConfig {
                log_dir: Some("/var/log/dh".to_string()),
                max_bytes: Some(2048),
                durability: Some("sync".to_string()),
            },
        };
        let resolved = resolve_config(Some(&fc), |key| match key {
            "CHROME_PATH" => Some("/opt/chrome".to_string()),
            "PROMPT_INJECTION_MODE" => Some("off".to_string()),
            "POLICY_FILE" => Some("/opt/policy.json".to_string()),
            "AUDIT_LOG_DIR" => Some("/tmp/audit".to_string()),
            "AUDIT_LOG_MAX_BYTES" => Some("4096".to_string()),
            "AUDIT_DURABILITY" => Some("flush".to_string()),
            _ => None,
        })
        .unwrap();
        assert_eq!(resolved.chrome_path, Some("/opt/chrome".to_string()));
        assert_eq!(resolved.injection_mode, PromptInjectionMode::Off);
        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["reveal developer message".to_string()]
        );
        assert_eq!(
            resolved.policy_file,
            Some(PathBuf::from("/opt/policy.json"))
        );
        assert_eq!(resolved.audit_log_dir, Some("/tmp/audit".to_string()));
        assert_eq!(resolved.audit_max_bytes, Some(4096));
        assert_eq!(resolved.audit_durability, Some("flush".to_string()));
    }

    #[test]
    fn resolve_config_additional_phrases_env_replaces_file_value() {
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                additional_phrases: vec!["from file".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let resolved = resolve_config(Some(&fc), |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
                .then(|| r#"["from env","second phrase"]"#.to_string())
        })
        .unwrap();

        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["from env".to_string(), "second phrase".to_string()]
        );
    }

    #[test]
    fn resolve_config_empty_additional_phrases_env_clears_file_value() {
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                additional_phrases: vec!["from file".to_string()],
                ..Default::default()
            },
            ..Default::default()
        };

        let resolved = resolve_config(Some(&fc), |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| "[]".to_string())
        })
        .unwrap();

        assert!(resolved.injection_additional_phrases.is_empty());
    }

    #[test]
    fn resolve_config_rejects_invalid_additional_phrases_env_without_echoing_value() {
        for raw in ["", r#"["secret phrase""#, r#"[{"secret":"phrase"}]"#] {
            let err = resolve_config(None, |key| {
                (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| raw.to_string())
            })
            .unwrap_err();
            assert!(matches!(err, ConfigError::InvalidAdditionalPhrasesFormat));
            if !raw.is_empty() {
                assert!(!err.to_string().contains(raw));
            }
            assert!(!err.to_string().contains("secret phrase"));
        }
    }

    #[test]
    fn resolve_config_normalizes_empty_and_duplicate_additional_phrases() {
        let resolved = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES)
                .then(|| r#"["  keep me  ","","keep me","   "]"#.to_string())
        })
        .unwrap();

        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["keep me".to_string()]
        );
    }

    #[test]
    fn resolve_config_normalizes_file_additional_phrases_preserving_order() {
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                additional_phrases: vec![
                    "  first phrase  ".to_string(),
                    "".to_string(),
                    "second phrase".to_string(),
                    "first phrase".to_string(),
                    "   ".to_string(),
                ],
                ..Default::default()
            },
            ..Default::default()
        };

        let resolved = resolve_config(Some(&fc), no_env).unwrap();

        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["first phrase".to_string(), "second phrase".to_string()]
        );
    }

    #[test]
    fn resolve_config_counts_effective_additional_phrases_after_normalization() {
        let raw = serde_json::to_string(
            &(0..=MAX_ADDITIONAL_PHRASES)
                .map(|_| " duplicate phrase ")
                .collect::<Vec<_>>(),
        )
        .unwrap();

        let resolved = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| raw.clone())
        })
        .unwrap();

        assert_eq!(
            resolved.injection_additional_phrases,
            vec!["duplicate phrase".to_string()]
        );
    }

    #[test]
    fn resolve_config_rejects_file_additional_phrase_limits_without_disclosure() {
        let secret = "file-secret-phrase";
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                additional_phrases: (0..=MAX_ADDITIONAL_PHRASES)
                    .map(|index| format!("{secret}-{index}"))
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let err = resolve_config(Some(&fc), no_env).unwrap_err();

        assert!(matches!(err, ConfigError::TooManyAdditionalPhrases { .. }));
        assert!(!err.to_string().contains(secret));
    }

    #[test]
    fn resolve_config_rejects_additional_phrase_resource_limits() {
        let too_many = serde_json::to_string(
            &(0..=MAX_ADDITIONAL_PHRASES)
                .map(|index| format!("phrase {index}"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let err = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| too_many.clone())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::TooManyAdditionalPhrases { .. }));

        let too_long =
            serde_json::to_string(&vec!["x".repeat(MAX_ADDITIONAL_PHRASE_BYTES + 1)]).unwrap();
        let err = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| too_long.clone())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::AdditionalPhraseTooLong { .. }));

        let count = MAX_ADDITIONAL_PHRASES_BYTES / MAX_ADDITIONAL_PHRASE_BYTES + 1;
        let too_large = serde_json::to_string(
            &(0..count)
                .map(|index| format!("{index:04}{}", "x".repeat(MAX_ADDITIONAL_PHRASE_BYTES - 4)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let err = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| too_large.clone())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::AdditionalPhrasesTooLarge { .. }));
    }

    #[test]
    fn resolve_config_accepts_additional_phrase_resource_boundaries() {
        let max_count = serde_json::to_string(
            &(0..MAX_ADDITIONAL_PHRASES)
                .map(|index| format!("phrase {index}"))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        let resolved = resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| max_count.clone())
        })
        .unwrap();
        assert_eq!(
            resolved.injection_additional_phrases.len(),
            MAX_ADDITIONAL_PHRASES
        );

        let exact_total = serde_json::to_string(
            &(0..(MAX_ADDITIONAL_PHRASES_BYTES / MAX_ADDITIONAL_PHRASE_BYTES))
                .map(|index| format!("{index:04}{}", "x".repeat(MAX_ADDITIONAL_PHRASE_BYTES - 4)))
                .collect::<Vec<_>>(),
        )
        .unwrap();
        resolve_config(None, |key| {
            (key == ENV_PROMPT_INJECTION_ADDITIONAL_PHRASES).then(|| exact_total.clone())
        })
        .unwrap();
    }

    #[test]
    fn resolve_config_applies_additional_phrase_limits_to_file_values() {
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                additional_phrases: (0..=MAX_ADDITIONAL_PHRASES)
                    .map(|index| format!("phrase {index}"))
                    .collect(),
                ..Default::default()
            },
            ..Default::default()
        };

        let err = resolve_config(Some(&fc), no_env).unwrap_err();
        assert!(matches!(err, ConfigError::TooManyAdditionalPhrases { .. }));
    }

    #[test]
    fn resolve_config_queries_every_registered_resolver_env_var() {
        use std::cell::RefCell;
        use std::collections::BTreeSet;

        let queried = RefCell::new(BTreeSet::new());
        resolve_config(None, |key| {
            queried.borrow_mut().insert(key.to_string());
            None
        })
        .unwrap();

        let expected = HONORED_CONFIG_ENV_VARS
            .iter()
            .copied()
            .filter(|key| *key != ENV_AUDIT_LOG_STDERR_MIRROR)
            .map(str::to_string)
            .collect::<BTreeSet<_>>();
        assert_eq!(*queried.borrow(), expected);
    }

    #[test]
    fn resolve_config_rejects_invalid_injection_mode_from_file() {
        let fc = FileConfig {
            prompt_injection: PromptInjectionFileConfig {
                mode: Some("redct".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = resolve_config(Some(&fc), no_env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidInjectionMode(ref m) if m == "redct"));
    }

    #[test]
    fn resolve_config_rejects_invalid_injection_mode_from_env() {
        let err = resolve_config(None, |key| {
            (key == "PROMPT_INJECTION_MODE").then(|| "verbose".to_string())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidInjectionMode(ref m) if m == "verbose"));
    }

    #[test]
    fn resolve_config_rejects_invalid_audit_durability_from_file() {
        let fc = FileConfig {
            audit: AuditFileConfig {
                durability: Some("eventual".to_string()),
                ..Default::default()
            },
            ..Default::default()
        };
        let err = resolve_config(Some(&fc), no_env).unwrap_err();
        assert!(matches!(err, ConfigError::InvalidAuditDurability(ref d) if d == "eventual"));
    }

    #[test]
    fn resolve_config_rejects_invalid_audit_durability_from_env() {
        let err = resolve_config(None, |key| {
            (key == "AUDIT_DURABILITY").then(|| "async".to_string())
        })
        .unwrap_err();
        assert!(matches!(err, ConfigError::InvalidAuditDurability(ref d) if d == "async"));
    }

    #[test]
    fn resolve_config_falls_back_when_audit_log_max_bytes_env_is_not_numeric() {
        let fc = FileConfig {
            audit: AuditFileConfig {
                max_bytes: Some(2048),
                ..Default::default()
            },
            ..Default::default()
        };
        let resolved = resolve_config(Some(&fc), |key| {
            (key == "AUDIT_LOG_MAX_BYTES").then(|| "not-a-number".to_string())
        })
        .unwrap();
        assert_eq!(resolved.audit_max_bytes, Some(2048));
    }
}
