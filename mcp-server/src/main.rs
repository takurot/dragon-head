use core_runtime::BrowserClient;
use mcp_server::{doctor, CoreRuntimeBackend, McpServer};
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

    let chrome_path = std::env::var("CHROME_PATH").ok();

    eprintln!("dragon-head-mcp: starting...");
    let client = BrowserClient::new_with_chrome_path(chrome_path)?;
    let page = client.new_page()?;
    let backend = CoreRuntimeBackend::new(page);
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
