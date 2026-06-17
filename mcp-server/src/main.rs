use anyhow::Context;
use core_runtime::audit::AuditLogger;
use core_runtime::{
    BrowserClient, PolicyEngine, PromptInjectionMode, PromptInjectionSanitizerConfig,
};
use mcp_server::{config, doctor, CoreRuntimeBackend, McpServer};
use std::io::{self, BufRead, Write};

mod cli;
mod init;

fn main() -> anyhow::Result<()> {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match cli::parse_args(&args) {
        cli::CliAction::Help => {
            println!("{}", cli::USAGE);
            return Ok(());
        }
        cli::CliAction::Version => {
            println!("dragon-head-mcp {}", env!("CARGO_PKG_VERSION"));
            return Ok(());
        }
        cli::CliAction::Init(client) => {
            if !init::print_init(client.as_deref()) {
                std::process::exit(1);
            }
            return Ok(());
        }
        cli::CliAction::Doctor => {
            let report = doctor::run_doctor();
            doctor::print_report(&report);
            if !report.all_passed() {
                std::process::exit(1);
            }
            return Ok(());
        }
        cli::CliAction::UnknownFlag(flag) => {
            eprintln!("dragon-head-mcp: unrecognized flag '{flag}'\n");
            eprintln!("{}", cli::USAGE);
            std::process::exit(2);
        }
        cli::CliAction::RunServer => {}
    }

    let env_lookup = |key: &str| std::env::var(key).ok();

    let config_path = config::default_config_path_with(env_lookup);
    let file_config = match &config_path {
        Some(path) => config::load_config_file(path)
            .with_context(|| format!("failed to load config file {}", path.display()))?,
        None => None,
    };
    let resolved = config::resolve_config(file_config.as_ref(), env_lookup)
        .context("failed to resolve dragon-head-mcp configuration")?;

    if resolved.injection_mode != PromptInjectionMode::ReportOnly {
        eprintln!(
            "[SECURITY][WARN] prompt_injection mode is {:?} (default: ReportOnly). \
             Indirect prompt-injection content may reach the agent unflagged.",
            resolved.injection_mode
        );
    }

    let audit_lookup = {
        let resolved = resolved.clone();
        move |key: &str| -> Option<String> {
            std::env::var(key).ok().or_else(|| match key {
                "AUDIT_LOG_DIR" => resolved.audit_log_dir.clone(),
                "AUDIT_LOG_MAX_BYTES" => resolved.audit_max_bytes.map(|bytes| bytes.to_string()),
                "AUDIT_DURABILITY" => resolved.audit_durability.clone(),
                _ => None,
            })
        }
    };
    let audit_logger = AuditLogger::from_env_with(audit_lookup);

    eprintln!("dragon-head-mcp: starting...");
    let client = BrowserClient::new_with_chrome_path(resolved.chrome_path.clone())?;
    let page = client.new_page_with_audit_logger(audit_logger)?;

    let mut backend = CoreRuntimeBackend::new_with_client(client, page);

    if let Some(policy_path) = &resolved.policy_file {
        let engine = PolicyEngine::try_from_file(policy_path)
            .with_context(|| format!("failed to load policy file {}", policy_path.display()))?;
        backend.set_policy_rules(engine.rules().to_vec())?;
    }

    backend.set_injection_config(PromptInjectionSanitizerConfig {
        mode: resolved.injection_mode,
        additional_phrases: resolved.injection_additional_phrases.clone(),
    });
    let mut server = McpServer::new(backend);
    eprintln!("dragon-head-mcp: ready, listening on stdio");

    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdout_handle = stdout.lock();

    for line in stdin.lock().lines() {
        let request = match line {
            Ok(line) => {
                let trimmed = line.trim().to_string();
                if trimmed.is_empty() {
                    continue;
                }
                trimmed
            }
            Err(err) => {
                eprintln!("dragon-head-mcp: stdin read error: {err}");
                break;
            }
        };

        if let Some(response) = server.handle_jsonrpc(&request) {
            writeln!(stdout_handle, "{response}")?;
            stdout_handle.flush()?;
        }
    }

    eprintln!("dragon-head-mcp: shutting down");
    Ok(())
}
