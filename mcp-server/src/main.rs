use anyhow::Context;
use core_runtime::audit::AuditLogger;
use core_runtime::{
    BrowserClient, PolicyEngine, PromptInjectionMode, PromptInjectionSanitizerConfig,
};
use mcp_server::{config, doctor, CoreRuntimeBackend, McpServer};
use serde_json::json;
use std::io::{self, BufRead, Read, Write};

mod cli;
mod init;

/// Caps a single stdio JSON-RPC request line so a misbehaving or malicious
/// client cannot drive unbounded memory growth before `handle_jsonrpc` sees
/// a single byte of the payload.
const MAX_REQUEST_LINE_BYTES: usize = 10 * 1024 * 1024;

enum StdinLine {
    Line(Vec<u8>),
    TooLong,
    Eof,
}

/// Reads one line from `reader`, capping growth at `max_bytes`. If a
/// newline isn't found within the cap, the remainder of that oversized line
/// is drained so the next call starts at the following line, and
/// `TooLong` is returned instead of buffering further. The line is returned
/// as raw bytes: decoding invalid UTF-8 is the caller's concern, and it
/// should be rejected rather than silently rewritten.
fn read_capped_line<R: BufRead>(reader: &mut R, max_bytes: usize) -> io::Result<StdinLine> {
    let mut buf = Vec::new();
    let read = (&mut *reader)
        .take(max_bytes as u64)
        .read_until(b'\n', &mut buf)?;
    if read == 0 {
        return Ok(StdinLine::Eof);
    }
    if buf.last() == Some(&b'\n') || read < max_bytes {
        // Either a complete line, or the reader hit true EOF before the cap.
        return Ok(StdinLine::Line(buf));
    }

    loop {
        let mut discard = Vec::new();
        let drained = (&mut *reader)
            .take(max_bytes as u64)
            .read_until(b'\n', &mut discard)?;
        if drained == 0 || discard.last() == Some(&b'\n') {
            break;
        }
    }
    Ok(StdinLine::TooLong)
}

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
    config::validate_unicode_additional_phrases_env()
        .context("failed to resolve dragon-head-mcp configuration")?;
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
                config::ENV_AUDIT_LOG_DIR => resolved.audit_log_dir.clone(),
                config::ENV_AUDIT_LOG_MAX_BYTES => {
                    resolved.audit_max_bytes.map(|bytes| bytes.to_string())
                }
                config::ENV_AUDIT_DURABILITY => resolved.audit_durability.clone(),
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
    let mut stdin_handle = stdin.lock();

    loop {
        let request = match read_capped_line(&mut stdin_handle, MAX_REQUEST_LINE_BYTES) {
            Ok(StdinLine::Eof) => break,
            Ok(StdinLine::TooLong) => {
                eprintln!(
                    "dragon-head-mcp: rejected request line exceeding \
                     {MAX_REQUEST_LINE_BYTES} bytes"
                );
                let response = json!({
                    "jsonrpc": "2.0",
                    "id": null,
                    "error": {
                        "code": -32600,
                        "message": format!(
                            "request line exceeds maximum size of {MAX_REQUEST_LINE_BYTES} bytes"
                        )
                    }
                });
                writeln!(stdout_handle, "{response}")?;
                stdout_handle.flush()?;
                continue;
            }
            Ok(StdinLine::Line(bytes)) => {
                let line = match String::from_utf8(bytes) {
                    Ok(line) => line,
                    Err(err) => {
                        eprintln!("dragon-head-mcp: rejected non-UTF-8 request line: {err}");
                        let response = json!({
                            "jsonrpc": "2.0",
                            "id": null,
                            "error": {
                                "code": -32700,
                                "message": "request line is not valid UTF-8"
                            }
                        });
                        writeln!(stdout_handle, "{response}")?;
                        stdout_handle.flush()?;
                        continue;
                    }
                };
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;

    #[test]
    fn reads_a_normal_line() {
        let mut reader = Cursor::new(b"hello\nworld\n".to_vec());
        match read_capped_line(&mut reader, 1024).unwrap() {
            StdinLine::Line(line) => assert_eq!(line, b"hello\n"),
            other => panic!(
                "expected Line, got {other:?}",
                other = std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn reports_eof_on_empty_input() {
        let mut reader = Cursor::new(Vec::new());
        assert!(matches!(
            read_capped_line(&mut reader, 1024).unwrap(),
            StdinLine::Eof
        ));
    }

    #[test]
    fn final_line_shorter_than_cap_without_trailing_newline_is_returned() {
        let mut reader = Cursor::new(b"short".to_vec());
        match read_capped_line(&mut reader, 1024).unwrap() {
            StdinLine::Line(line) => assert_eq!(line, b"short"),
            other => panic!(
                "expected Line, got {other:?}",
                other = std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn rejects_oversized_line_and_resyncs_to_the_next_line() {
        let oversized = "a".repeat(50);
        let input = format!("{oversized}\nshort\n");
        let mut reader = Cursor::new(input.into_bytes());

        assert!(matches!(
            read_capped_line(&mut reader, 10).unwrap(),
            StdinLine::TooLong
        ));

        match read_capped_line(&mut reader, 10).unwrap() {
            StdinLine::Line(line) => assert_eq!(line, b"short\n"),
            other => panic!(
                "expected Line after resync, got {other:?}",
                other = std::mem::discriminant(&other)
            ),
        }
    }

    #[test]
    fn does_not_grow_the_buffer_past_the_cap_for_a_single_oversized_line() {
        // 50 MiB with no newline: if this test completes quickly and without
        // OOM, the read was bounded rather than buffering the whole line.
        let oversized = vec![b'a'; 50 * 1024 * 1024];
        let mut reader = Cursor::new(oversized);

        assert!(matches!(
            read_capped_line(&mut reader, 1024).unwrap(),
            StdinLine::TooLong
        ));
    }

    #[test]
    fn preserves_invalid_utf8_bytes_for_the_caller_to_reject() {
        // read_capped_line must not lossily rewrite invalid UTF-8 — the
        // caller decodes with `String::from_utf8` and rejects the request
        // instead of silently substituting replacement characters.
        let mut reader = Cursor::new(vec![0xFF, 0xFE, b'\n']);
        match read_capped_line(&mut reader, 1024).unwrap() {
            StdinLine::Line(line) => {
                assert_eq!(line, vec![0xFF, 0xFE, b'\n']);
                assert!(String::from_utf8(line).is_err());
            }
            other => panic!(
                "expected Line, got {other:?}",
                other = std::mem::discriminant(&other)
            ),
        }
    }
}
