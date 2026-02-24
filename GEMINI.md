# GEMINI.md - Dragon Head: Neural-Browser Runtime

## Project Overview
**Dragon Head** is an AI-native headless browser runtime designed for Large Language Models (LLMs) and Vision Language Models (VLMs). It acts as middleware, converting traditional web DOM into a "Semantic State" optimized for AI consumption, significantly reducing token usage and improving interaction reliability.

### Key Architecture
The project follows a 3-layer architecture:
1.  **Core Runtime (Rust/C++)**: Wraps Chromium CDP to handle rendering, Semantic Rendering Engine (SRE) conversion, and security controls.
2.  **Plugin Framework (WebAssembly)**: Sandbox environment for extending state extraction and policies.
3.  **Skills Engine**: Execution engine for declarative workflows (e.g., "Purchase Item").

### Core Technologies
- **Language**: Rust (Workspace with `core-runtime`, `plugin-host`, `skills-engine`).
- **Browser Control**: Chromium via Chrome DevTools Protocol (CDP).
- **Async Runtime**: `tokio`.
- **Serialization**: `serde`, `serde_json`.
- **Task Runner**: `just`.

## Building and Running
The project uses `just` as a command runner for common development tasks.

| Command | Description |
| :--- | :--- |
| `just check` | Runs `cargo check` across the entire workspace. |
| `just build` | Builds the project (standard `cargo build` is used). |
| `just fmt` | Formats all code using `cargo fmt`. |
| `just lint` | Runs `clippy` with strict warnings enabled (`-D warnings`). |
| `just test` | Executes all unit and integration tests. |
| `just test-all` | Runs tests, linting, and formatting checks. |

*Prerequisites: Rust (latest stable) and Chromium must be installed.*

## Development Conventions
- **Testing Pyramid**: Adheres to a structured testing strategy:
  - **Unit Tests**: Co-located in `src/` or `tests/unit/`.
  - **Integration Tests**: Located in `tests/integration/`.
  - **E2E Tests**: Located in `tests/e2e/`, using real Chromium instances.
- **Progress Tracking**: Progress and task statuses are tracked in `docs/PLAN.md`.
- **Code Style**: Strictly enforced via `clippy` and `fmt`. Ensure all PRs pass `just test-all`.
- **SRE (Semantic Rendering Engine)**: Focuses on deterministic state generation and token efficiency.
- **Stable Identity**: Uses `stable_key` (SHA-256 based) for robust element identification across re-renders.
- **Security**: Includes a Policy Engine for action screening and PII redaction in audit logs.

## Key Directories
- `core-runtime/`: The heart of the system, implementing CDP wrapping and SRE.
- `plugin-host/`: Wasm-based extensibility layer.
- `skills-engine/`: Declarative task execution engine.
- `docs/`: Comprehensive documentation including `SPEC.md`, `PLAN.md`, and `testing.md`.
- `.agent/skills/`: Custom agent skills for development workflows.
