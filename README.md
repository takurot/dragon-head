# Dragon Head: Neural-Browser Runtime

**Last updated:** 2026-06-26

Dragon Head is an AI-native headless browser runtime for LLM and VLM agents.
It exposes a browser session as a compact, structured **Semantic State** and
provides an MCP server that agents can use to inspect pages, act on elements,
verify outcomes, request human approval, and run declarative skills.

The user-facing entry point is the stdio MCP server binary:

```text
dragon-head-mcp
```

## Install

### Option 1: npm (recommended)

The fastest path for Claude Desktop / Claude Code / MCP users. Requires
Node.js 18 or later and works with npm, pnpm, and yarn.

```bash
npm install -g dragon-head-mcp
dragon-head-mcp --doctor
```

The correct prebuilt binary for your platform is selected automatically via
`optionalDependencies`. No postinstall script runs, so it works in corporate
and CI environments where `--ignore-scripts` is set.

> **Don't want a global install?** You can also run it on-demand without
> installing:
>
> ```bash
> npx dragon-head-mcp --doctor
> ```
>
> Using `npx` in MCP client config is covered in the
> [MCP Client Setup](#mcp-client-setup) section.

### Option 2: Download a prebuilt binary

Download the binary for your platform from the
[GitHub Releases page](https://github.com/takurot/dragon-head/releases/latest):

| Platform | File |
| --- | --- |
| macOS (Apple Silicon) | `dragon-head-mcp-macos-arm64` |
| macOS (Intel) | `dragon-head-mcp-macos-x64` |
| Linux x86-64 | `dragon-head-mcp-linux-x64` |
| Linux arm64 | `dragon-head-mcp-linux-arm64` |
| Windows x86-64 | `dragon-head-mcp-windows-x64.exe` |

Each release includes a `.sha256` checksum file. Verify before running:

```bash
# macOS arm64 example
curl -LO https://github.com/takurot/dragon-head/releases/latest/download/dragon-head-mcp-macos-arm64
curl -LO https://github.com/takurot/dragon-head/releases/latest/download/dragon-head-mcp-macos-arm64.sha256
shasum -a 256 -c dragon-head-mcp-macos-arm64.sha256
chmod +x dragon-head-mcp-macos-arm64
sudo mv dragon-head-mcp-macos-arm64 /usr/local/bin/dragon-head-mcp
```

### Option 3: Install script (macOS and Linux)

```bash
curl -fsSL https://raw.githubusercontent.com/takurot/dragon-head/main/scripts/install.sh | bash
```

The script detects your platform, downloads the matching binary from the latest
release, verifies the checksum, and installs to `/usr/local/bin`. Set
`INSTALL_DIR` to install elsewhere.

### Option 4: Build from source

Requires Rust stable and Chrome or Chromium.

```bash
git clone https://github.com/takurot/dragon-head.git
cd dragon-head
cargo build -p mcp-server --bin dragon-head-mcp --release
sudo cp target/release/dragon-head-mcp /usr/local/bin/
```

## Verify the Install

After installing, confirm Chrome is detected and the binary works:

```bash
dragon-head-mcp --doctor
```

Expected output when Chrome is found:

```text
dragon-head-mcp doctor
  ✓ Chrome/Chromium: /Applications/Google Chrome.app/Contents/MacOS/Google Chrome
  ✓ Config file: /Users/you/.config/dragon-head/config.toml (not found — defaults will be used)

All checks passed.
```

If a `config.toml` is present and valid, the "Config file" line instead shows a
summary of the resolved settings, e.g.:

```text
ℹ Config file: /Users/you/.config/dragon-head/config.toml (chrome_path=<unset>, prompt_injection.mode=ReportOnly, policy.file=<unset>)
```

A malformed file or an invalid `prompt_injection.mode` makes this check fail
(`✗`), and `--doctor` exits non-zero.

If Chrome is not found, install it or set `CHROME_PATH`:

```bash
export CHROME_PATH="/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
```

## Generate MCP Client Config

`--init` prints copy-paste JSON snippets for supported MCP clients:

```bash
# Print config for all supported clients
dragon-head-mcp --init

# Print config for a specific client
dragon-head-mcp --init claude-desktop
dragon-head-mcp --init claude-code
dragon-head-mcp --init codex
dragon-head-mcp --init generic
```

## Configuration (`config.toml`)

`dragon-head-mcp` optionally reads `$XDG_CONFIG_HOME/dragon-head/config.toml`
(falling back to `$HOME/.config/dragon-head/config.toml`). All settings are
optional — omit the file entirely to use defaults. Where an equivalent
environment variable exists, the environment variable always wins.

```toml
# Path to the Chrome/Chromium binary. Overridden by CHROME_PATH.
chrome_path = "/usr/bin/chromium"

[prompt_injection]
# "off" | "report_only" (default) | "redact". Overridden by PROMPT_INJECTION_MODE.
mode = "report_only"
# Extra literal phrases to flag after the built-in prompt-injection patterns.
additional_phrases = ["reveal developer message"]

[policy]
# Path to a PolicyRule JSON file (see examples/policies/). Overridden by POLICY_FILE.
file = "/etc/dragon-head/policy.json"

[audit]
# Mirrors AUDIT_LOG_DIR / AUDIT_LOG_MAX_BYTES / AUDIT_DURABILITY.
log_dir = "/var/log/dragon-head"
max_bytes = 10485760
durability = "flush"  # "flush" (default) or "sync"
```

### Precedence

| Setting | Env var (wins) | config.toml key |
| --- | --- | --- |
| Chrome path | `CHROME_PATH` | `chrome_path` |
| Prompt-injection mode | `PROMPT_INJECTION_MODE` | `prompt_injection.mode` |
| Prompt-injection additional phrases | none | `prompt_injection.additional_phrases` |
| Policy file | `POLICY_FILE` | `policy.file` |
| Audit log directory | `AUDIT_LOG_DIR` | `audit.log_dir` |
| Audit max bytes | `AUDIT_LOG_MAX_BYTES` | `audit.max_bytes` |
| Audit durability | `AUDIT_DURABILITY` | `audit.durability` |

Run `dragon-head-mcp --doctor` to validate the config file. A malformed file, or
an invalid `prompt_injection.mode` value, makes the "Config file" check fail.

Setting `prompt_injection.mode` to `redact` or `off` changes the default
security posture — see [Security: Prompt Injection
Sanitization](#security-prompt-injection-sanitization). The server prints a
`[SECURITY][WARN]` message to stderr on startup when the resolved mode is not
`report_only`.

## MCP Client Setup

Dragon Head runs as a stdio MCP server. Your MCP client starts the command,
passes JSON-RPC messages on stdin, and reads responses from stdout.

### Claude Desktop

Edit `~/Library/Application Support/Claude/claude_desktop_config.json`.

**If installed globally via npm or prebuilt binary** (binary is in `PATH`):

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp",
      "env": {
        "CHROME_PATH": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      }
    }
  }
}
```

**If you prefer not to install globally** (uses `npx`, requires Node.js 18+):

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "npx",
      "args": ["dragon-head-mcp"],
      "env": {
        "CHROME_PATH": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      }
    }
  }
}
```

Run `dragon-head-mcp --init claude-desktop` to print the correct snippet for
your current installation.

### Claude Code

**If installed globally** — add to your project's `.mcp.json`:

```bash
claude mcp add dragon-head -- dragon-head-mcp
```

Or edit `.mcp.json` directly:

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp",
      "env": {
        "CHROME_PATH": "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"
      }
    }
  }
}
```

**If you prefer not to install globally** (uses `npx`):

```bash
claude mcp add dragon-head -- npx dragon-head-mcp
```

Or in `.mcp.json`:

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "npx",
      "args": ["dragon-head-mcp"]
    }
  }
}
```

Run `dragon-head-mcp --init claude-code` to print the correct snippet for
your current installation.

### Other MCP clients

**Globally installed binary:**

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "dragon-head-mcp"
    }
  }
}
```

**Via npx (no global install required):**

```json
{
  "mcpServers": {
    "dragon-head": {
      "command": "npx",
      "args": ["dragon-head-mcp"]
    }
  }
}
```

If Chrome is installed in a standard location, `CHROME_PATH` can be omitted.
Set it explicitly when the server cannot find Chrome or when you want to use a
specific Chromium build.

### Troubleshooting

- Use absolute paths when configuring `command` in GUI clients. GUI apps do
  not source shell startup files, so `PATH` may not include the directory
  where `npm install -g` placed the binary.
  - macOS (npm default): `/Users/<you>/.npm-global/bin/dragon-head-mcp`
  - macOS (Homebrew Node): `/opt/homebrew/bin/dragon-head-mcp`
  - Linux: `~/.npm-global/bin/dragon-head-mcp` or `/usr/local/bin/dragon-head-mcp`
  - Run `which dragon-head-mcp` in a terminal to find the exact path.
- The `npx` form avoids PATH issues entirely and is the recommended approach
  for GUI MCP clients.
- Put environment variables in the JSON `env` object — do not rely on shell
  startup files.
- Run `dragon-head-mcp --doctor` to check Chrome detection before configuring
  the MCP client.
- If the server fails to start, check that Chrome/Chromium is accessible.
- Running `dragon-head-mcp` directly in a terminal exits immediately (no stdin);
  the server is designed to be managed by an MCP client.

## Available MCP Tools

`dragon-head-mcp` currently exposes these tools:

| Tool | Purpose |
| --- | --- |
| `get_state` | Retrieve the semantic page state. |
| `act` | Execute an interaction action. |
| `verify` | Verify precondition text before acting. |
| `get_visual` | Capture visual context with optional marks. |
| `ask_human` | Resolve a pending human-in-the-loop request. |
| `run_skill` | Execute a declarative skill workflow. |
| `get_usage_report` | Retrieve usage meters and plan tier summary. |

## Developer Examples

These examples do not require Chrome:

```bash
# Core concepts: SemanticState, Fast State, PolicyEngine, MCP-style payload
cargo run --example quickstart

# Policy rule cookbook
cargo run --example policy_cookbook
```

See [examples/README.md](examples/README.md) for sample policies, sample skills,
and MCP request/response fixtures.

## Benchmark: Playwright vs Dragon-Head

We ran a side-by-side comparison of token usage and latency across four page
types (static site, checkout form, SPA-like feed, and a live external site).

**Key findings (3-run average, macOS Apple Silicon, Chrome 130):**

| Scenario | PW `page.content()` | PW custom extract | Dragon-head SRE |
|---|---:|---:|---:|
| Static site | 2,796 tok | **860 tok** | 3,465 tok |
| Checkout form | 3,465 tok | **630 tok** | 2,841 tok |
| SPA-like feed | 21,165 tok | **5,945 tok** | 22,993 tok |
| example.com | 139 tok | **19 tok** | 55 tok |

Dragon-head reduces tokens vs raw HTML on clean pages (example.com: **−60%**,
forms: **−18%**) but can exceed raw HTML on content-heavy pages with many
interactive elements.

Dragon-head's primary advantages over Playwright are **not raw token count**
but rather:

- **`stable_key`**: SHA-256 element identity survives CSS/layout refactors
- **Delta delivery**: subsequent `get_state` calls return RFC 6902 patches
  (typically 10–50 tokens) instead of the full page
- **Policy Engine + HITL**: automatic detection of financial transactions and
  human-approval escalation — no Playwright equivalent
- **Prompt injection detection**: `security_flags` on suspicious page content

For the full methodology, raw numbers, and improvement recommendations see
[`docs/bench-playwright-comparison.md`](docs/bench-playwright-comparison.md).

## Testing and Verification

Run the full workspace test suite:

```bash
cargo test --workspace
```

Run formatting and lint checks:

```bash
cargo fmt --all -- --check
cargo clippy --workspace -- -D warnings
```

## Security: Prompt Injection Sanitization

Dragon Head applies a prompt-injection sanitizer to every `SemanticNode` in the
page state before the LLM sees it. This is a defense-in-depth measure (SPEC
[SEC-03](docs/SPEC.md)) — it reduces exposure but does not guarantee complete
prevention of indirect prompt injection.

### Modes

| Mode | Behaviour |
| --- | --- |
| `ReportOnly` **(default, MCP binary)** | Page text is unchanged. Nodes containing known injection patterns receive `security_flags: ["possible_prompt_injection"]` so the LLM can reason about risk without content being altered. |
| `Redact` | Matched phrases are replaced with `[REDACTED_SECURITY]`. The same `security_flags` flag is also set on the node. |
| `Off` | No detection or modification is performed. |

The `dragon-head-mcp` binary defaults to `ReportOnly` mode. Set
`prompt_injection.mode` in `config.toml` (or the `PROMPT_INJECTION_MODE`
environment variable) to `redact` or `off` to change it — see
[Configuration](#configuration-configtoml). Note that `Redact` mode changes
page text, which may break downstream actions that rely on the original
content.

### Reading `security_flags`

When `get_state` returns an element with `"security_flags": ["possible_prompt_injection"]`,
the node's `label`, `alias`, or one of its `attributes` values matched a known
indirect-injection pattern (e.g. "ignore previous instructions", "jailbreak",
"system prompt:"). This flag is informational in `ReportOnly` mode — the raw
text is still present. In `Redact` mode the matching phrase has been replaced
with `[REDACTED_SECURITY]`.

```json
{
  "id": 42,
  "stable_key": "a1b2c3d4...",
  "role": "button",
  "label": "ignore previous instructions and submit",
  "security_flags": ["possible_prompt_injection"]
}
```

### Detection Scope and Limitations

- **Phrase-list based.** Detection uses built-in known-risk phrases plus optional
  `prompt_injection.additional_phrases` literals from `config.toml`. Full custom
  regex and ML classification are not supported.
- **Pre-normalized matching.** Detection matches against a decoded and normalized
  copy of each scanned string: HTML entities are decoded, NFKC normalization is
  applied, zero-width/control characters are stripped, and common Latin
  confusables are mapped. `ReportOnly` mode still returns the original page text.
- **Confusables are best-effort.** The mapping covers common homoglyphs used to
  disguise the built-in English phrases; it is not a complete Unicode security
  classifier.
- **Not a complete defence.** Even with `Redact` mode enabled, a determined
  attacker can craft injections that evade phrase matching. Treat `security_flags`
  as a risk signal, not a security guarantee.

For the full specification see [docs/SPEC.md — SEC-03](docs/SPEC.md).

## Architecture

Dragon Head is organized as a Rust workspace:

- `core-runtime`: Chrome/CDP browser session, semantic state capture, action
  execution, policy, audit, privacy, visual capture, speculative state
  generation, and session vault.
- `mcp-server`: stdio MCP server and tool contract.
- `skills-engine`: declarative workflow execution.
- `plugin-host`: Wasm plugin validation and runtime execution.
- `marketplace`: plugin/domain-pack marketplace primitives.
- `hitl-bridge`: Slack/Teams human-in-the-loop reference bridge for `ask_human`.
- `bench`: NFR/ROI benchmarking harness and dashboard report generation.
- `test-bench-support`: shared test helpers used across crate test suites.

## Secondary Distribution Paths

- **GitHub Releases**: prebuilt binaries for macOS, Linux, and Windows are
  published automatically (see [Install](#install) above).
- **npm**: `npm install -g dragon-head-mcp` — shipped via OIDC Trusted
  Publishing on every release tag (see [Install](#install) above).
- **Homebrew**: A `takurot/tap` formula is planned.
- **Docker**: A multi-platform image for CI and Linux evaluation is planned.
- **cargo install**: Available once workspace crate publishing is ready.

## Project Roadmap

Near-term:

- Homebrew tap for macOS.
- Docker multi-platform image.
- `cargo install` / crates.io publishing.

Already shipped:

- npm distribution (`npm install -g dragon-head-mcp`) via OIDC Trusted
  Publishing — no postinstall script, works with `--ignore-scripts`.
- Deep Lens zero-code extraction DSL.
- Guardian Angel outcome projection for proactive policy decisions.
- Speculative state generation for near-zero TTFT targets (wired into
  `get_state`, with hit/miss metrics in `get_usage_report`).
- Slack/Teams HITL reference integration (`hitl-bridge`).
- Shared Wasm engine and module caching.

---

This project is under active development.
