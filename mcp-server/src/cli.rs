//! Command-line argument parsing for the `dragon-head-mcp` binary.

/// Usage text shown for `--help`/`-h` and on unrecognized-flag errors.
pub const USAGE: &str = "\
dragon-head-mcp - AI-native headless browser runtime MCP server

USAGE:
    dragon-head-mcp [FLAGS]

FLAGS:
    --doctor          Check Chrome detection and configuration, then exit
    --init [CLIENT]   Print an MCP client config snippet and exit
                      CLIENT: claude-code, claude-desktop, codex, generic
    -V, --version     Print version information and exit
    -h, --help        Print this help message and exit

With no flags, starts the stdio MCP server.";

/// The action requested via command-line arguments.
#[derive(Debug, PartialEq, Eq)]
pub enum CliAction {
    /// Start the stdio MCP server (default).
    RunServer,
    /// Run Chrome/config diagnostics and exit.
    Doctor,
    /// Print an MCP client config snippet for the given client (or all clients).
    Init(Option<String>),
    /// Print version information and exit.
    Version,
    /// Print usage information and exit.
    Help,
    /// An unrecognized `--flag` was passed.
    UnknownFlag(String),
}

/// Parses CLI arguments (excluding the program name) into a [`CliAction`].
pub fn parse_args(args: &[String]) -> CliAction {
    if args.iter().any(|a| a == "--help" || a == "-h") {
        return CliAction::Help;
    }

    if args.iter().any(|a| a == "--version" || a == "-V") {
        return CliAction::Version;
    }

    if args.iter().any(|a| a == "--doctor") {
        return CliAction::Doctor;
    }

    if let Some(pos) = args.iter().position(|a| a == "--init") {
        let client = args.get(pos + 1).filter(|s| !s.starts_with('-')).cloned();
        return CliAction::Init(client);
    }

    if let Some(flag) = args.iter().find(|a| a.starts_with("--")) {
        return CliAction::UnknownFlag(flag.clone());
    }

    CliAction::RunServer
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn no_args_runs_server() {
        assert_eq!(parse_args(&args(&[])), CliAction::RunServer);
    }

    #[test]
    fn doctor_flag_returns_doctor() {
        assert_eq!(parse_args(&args(&["--doctor"])), CliAction::Doctor);
    }

    #[test]
    fn init_without_client_returns_init_none() {
        assert_eq!(parse_args(&args(&["--init"])), CliAction::Init(None));
    }

    #[test]
    fn init_with_client_returns_init_some() {
        assert_eq!(
            parse_args(&args(&["--init", "claude-code"])),
            CliAction::Init(Some("claude-code".to_string()))
        );
    }

    #[test]
    fn doctor_takes_precedence_over_init() {
        assert_eq!(
            parse_args(&args(&["--init", "--doctor"])),
            CliAction::Doctor
        );
    }

    #[test]
    fn init_does_not_consume_following_flag_as_client() {
        assert_eq!(
            parse_args(&args(&["--init", "--bogus"])),
            CliAction::Init(None)
        );
    }

    #[test]
    fn version_long_flag_returns_version() {
        assert_eq!(parse_args(&args(&["--version"])), CliAction::Version);
    }

    #[test]
    fn version_short_flag_returns_version() {
        assert_eq!(parse_args(&args(&["-V"])), CliAction::Version);
    }

    #[test]
    fn help_long_flag_returns_help() {
        assert_eq!(parse_args(&args(&["--help"])), CliAction::Help);
    }

    #[test]
    fn help_short_flag_returns_help() {
        assert_eq!(parse_args(&args(&["-h"])), CliAction::Help);
    }

    #[test]
    fn unknown_flag_is_rejected() {
        assert_eq!(
            parse_args(&args(&["--bogus"])),
            CliAction::UnknownFlag("--bogus".to_string())
        );
    }

    #[test]
    fn help_takes_precedence_over_other_flags() {
        assert_eq!(parse_args(&args(&["--doctor", "--help"])), CliAction::Help);
    }
}
