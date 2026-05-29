pub const CLIENTS: &[&str] = &["claude-desktop", "claude-code", "codex", "generic"];

/// Returns a copy-paste JSON config snippet for the given client, or None for unknown clients.
pub fn config_snippet(client: &str) -> Option<String> {
    match client {
        "claude-desktop" => Some(claude_desktop_snippet()),
        "claude-code" => Some(claude_code_snippet()),
        "codex" => Some(codex_snippet()),
        "generic" => Some(generic_snippet()),
        _ => None,
    }
}

/// Prints MCP client config templates to stdout.
/// `client = None` prints all clients; `client = Some(name)` prints just that one.
/// Returns false if the client name is unknown.
pub fn print_init(client: Option<&str>) -> bool {
    match client {
        None => {
            for &name in CLIENTS {
                print_client(name);
                println!();
            }
            true
        }
        Some(name) => {
            if let Some(snippet) = config_snippet(name) {
                print_client_with_snippet(name, &snippet);
                true
            } else {
                eprintln!(
                    "dragon-head-mcp --init: unknown client '{name}'. Available: {}",
                    CLIENTS.join(", ")
                );
                false
            }
        }
    }
}

fn print_client(name: &str) {
    if let Some(snippet) = config_snippet(name) {
        print_client_with_snippet(name, &snippet);
    }
}

fn print_client_with_snippet(name: &str, snippet: &str) {
    let description = match name {
        "claude-desktop" => {
            "Add to ~/Library/Application Support/Claude/claude_desktop_config.json"
        }
        "claude-code" => "Run: claude mcp add dragon-head -- dragon-head-mcp\nOr add to .mcp.json:",
        "codex" => "Add to ~/.codex/config.toml (or project .codex/config.toml):",
        "generic" => "Any MCP client that accepts mcpServers:",
        _ => "Add to your MCP client config:",
    };
    println!("# {name}: {description}");
    println!("{snippet}");
}

fn claude_desktop_snippet() -> String {
    r#"{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp"
    }
  }
}"#
    .to_owned()
}

fn claude_code_snippet() -> String {
    r#"{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp"
    }
  }
}"#
    .to_owned()
}

fn codex_snippet() -> String {
    // Codex CLI uses TOML config (not JSON), with mcp_servers array entries.
    r#"[[mcp_servers]]
name = "dragon-head"
command = "dragon-head-mcp"
args = []"#
        .to_owned()
}

fn generic_snippet() -> String {
    r#"{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp"
    }
  }
}"#
    .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_has_command(snippet: &str) {
        let v: serde_json::Value =
            serde_json::from_str(snippet).expect("snippet must be valid JSON");
        assert_eq!(
            v["mcpServers"]["dragon-head"]["command"], "dragon-head-mcp",
            "snippet must set command to dragon-head-mcp"
        );
    }

    #[test]
    fn config_snippet_claude_desktop_has_command() {
        let s = config_snippet("claude-desktop").expect("claude-desktop is a known client");
        assert_has_command(&s);
    }

    #[test]
    fn config_snippet_claude_code_has_command() {
        let s = config_snippet("claude-code").expect("claude-code is a known client");
        assert_has_command(&s);
    }

    #[test]
    fn config_snippet_codex_is_toml_with_command() {
        let s = config_snippet("codex").expect("codex is a known client");
        // Codex uses TOML, not JSON — verify it contains the required TOML fields.
        assert!(
            s.contains("mcp_servers"),
            "codex snippet must use mcp_servers (TOML key)"
        );
        assert!(
            s.contains("dragon-head-mcp"),
            "codex snippet must reference dragon-head-mcp"
        );
    }

    #[test]
    fn config_snippet_generic_has_command() {
        let s = config_snippet("generic").expect("generic is a known client");
        assert_has_command(&s);
    }

    #[test]
    fn config_snippet_unknown_returns_none() {
        assert!(
            config_snippet("not-a-real-client").is_none(),
            "unknown client should return None"
        );
    }

    #[test]
    fn clients_constant_covers_all_known_names() {
        for &name in CLIENTS {
            let s = config_snippet(name);
            assert!(
                s.is_some(),
                "CLIENTS lists '{name}' but config_snippet returns None for it"
            );
            assert!(
                s.unwrap().contains("dragon-head-mcp"),
                "snippet for '{name}' must reference dragon-head-mcp"
            );
        }
    }

    #[test]
    fn print_init_none_returns_true() {
        assert!(print_init(None), "print_init(None) must return true");
    }

    #[test]
    fn print_init_known_client_returns_true() {
        assert!(
            print_init(Some("claude-desktop")),
            "print_init with known client must return true"
        );
    }

    #[test]
    fn print_init_unknown_client_returns_false() {
        assert!(
            !print_init(Some("not-a-real-client")),
            "print_init with unknown client must return false"
        );
    }
}
