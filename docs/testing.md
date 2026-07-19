# Testing Strategy

This document outlines the testing strategy for the Neural-Browser Runtime project.

## 1. Test Levels

We follow the "Testing Pyramid" approach with the following layers:

### 1.1 Unit Tests (Level 1)
- **Scope**: Individual functions, structs, and modules.
- **Tools**: Rust intrinsic `#[test]`, `pytest` (for Python bindings).
- **Location**: Rust unit tests live next to the code in each crate's `src/`;
  integration tests live in crate-local `tests/` directories.
- **Frequency**: Run on every file save (local), check on every commit (CI).
- **Coverage Target**: High (>80%).

### 1.2 Integration Tests (Level 2)
- **Scope**: Interaction between modules (e.g., SRE + CDP client).
- **Tools**: Rust `tests/` directory.
- **Location**: crate-local `tests/` directories such as `core-runtime/tests/`
  and `mcp-server/tests/`.
- **Frequency**: Run on every PR.
- **Mocking**: External services (CDP) may be mocked or run against a real headless browser docker container.

### 1.3 E2E Tests (Level 3)
- **Scope**: Full system verification (Client -> Runtime -> Browser -> Website).
- **Tools**: Custom runner, real Chromium instance.
- **Location**: browser-backed integration tests under crate-local `tests/`
  directories, plus workflow orchestration in `.github/workflows/e2e.yml`.
- **Frequency**: Run on PR (Smoke suite), Nightly (Full suite).

## 2. CI/CD Pipeline

The CI pipeline is defined in `.github/workflows/`.

- **`ci.yml`**: Runs `fmt`, `clippy`, unit tests, and integration tests.
- **MCP binary stdio smoke (PR required)**: `ci.yml` starts the shipped
  `dragon-head-mcp` binary with Chrome and verifies `initialize`,
  `notifications/initialized`, and `tools/list` over real stdio. The gate rejects
  stdout contamination, missing required tools, startup hangs, and unclean shutdowns.
- **Playwright comparison harness**: `ci.yml` installs and tests `bench-playwright` at the
  supported Node.js 20.19, 22.12, and 24 boundaries, and rejects moderate-or-higher npm
  audit findings.
- **Skill schema version gate**: `skill-schema-compatibility` rejects JSON definitions above
  the supported schema version, while `skill-conformance` rejects unsupported typed
  definitions before any runtime operation is invoked.
- **`e2e.yml`**: Runs E2E tests against a headless browser.
- **Performance Gate (PR Short)**: `ci.yml` runs a short NFR suite (`ttft`, `nfr_latency`, `nfr_bandwidth`, `nfr_capacity`) and generates `core-runtime/target/nfr-dashboard.md`.
- **Performance Gate (Nightly Full)**: `e2e.yml` runs the full NFR suite (including long TTFT and full capacity targets) and publishes the same dashboard format for regression tracking.
- **NFR Fidelity**: the workspace `test` profile keeps `incremental = false` and uses a modest optimization level so latency-oriented tests measure runtime behavior rather than debug-build artifacts.
- **Latency Scope**: `nfr_latency` measures the SRE update path after obtaining the current DOM snapshot, matching the spec's focus on semantic state regeneration under subtree refinement.
- **Threshold Enforcement**: `scripts/nfr_dashboard.py` evaluates metric thresholds from JSON outputs and fails CI on regressions.
- **Comprehensive Evaluation Bench**: `just evaluation-bench-smoke` runs crate-spanning feature evaluation suites and emits JSON reports plus `target/evaluation-dashboard.md`. Nightly/full runs use the same format with `DRAGON_HEAD_EVAL_MODE=full`.

## 2.1 Comprehensive Evaluation Bench

- **Goal**: Provide a single dashboard that verifies the main Dragon Head feature areas across `core-runtime`, `mcp-server`, `skills-engine`, `plugin-host`, and `marketplace`.
- **Modes**:
- `smoke`: Required on PRs. Covers representative scenarios for state capture, action recovery, wait semantics, policy/HITL, audit/session, MCP flow (including visual image content delivery and standalone configured-skill loading), skill execution, plugin validation, and marketplace accounting.
  - `full`: Runs on nightly/manual workflows. Uses the same report format and is reserved for expanded scenario sets and longer-running variants.
- **Artifacts**:
  - JSON reports: `target/evaluation-bench/*.json`
  - Markdown dashboard: `target/evaluation-dashboard.md`
- **Registration Rule**: New major features are not considered complete until a corresponding scenario is added to the evaluation bench or an explicit exemption is documented in `docs/`.

## 3. Running Tests Locally

### 3.1 Dev Container (Recommended)

The repository ships a `.devcontainer/` configuration that closely matches the CI environment
(Ubuntu 24.04, Google Chrome stable, Rust stable, cargo-nextest pinned to the same version as CI).
Open the repo in VS Code and choose **"Reopen in Container"** — or use the GitHub Codespaces button — to get a consistent test environment without manual setup.

```bash
# Inside the dev container or on a machine with Chrome and Rust installed:

# Run all unit and integration tests (same as CI)
cargo nextest run --workspace --profile ci

# Run doc tests
cargo test --workspace --doc

# Run a specific integration test
cargo test -p core-runtime --test sre_determinism --verbose

# Reproduce the skill schema version contract gates
cargo test -p skills-engine --test skill_schema_compatibility --verbose
cargo test -p skills-engine --test skill_conformance --verbose

# Run E2E tests (requires Chrome; CHROME_INSTALLED=true is set by the container)
CHROME_INSTALLED=true cargo test -p core-runtime --test semantic_wait
```

### 3.2 Manual Setup (without the container)

```bash
# Run all unit and integration tests
cargo test

# Compile the full workspace test suite without running it
cargo test --workspace --no-run

# Run specific test
cargo test test_name

# Run E2E tests (requires setup)
cargo test --test e2e
```

The workspace disables incremental compilation for the `test` profile so the default `cargo test --workspace` and `cargo test --workspace --no-run` flows remain stable on filesystems where incremental dep-graph artifact creation is unreliable.

## 3.3 Binary Fixture Policy (LFS)

Test fixtures are stored under `<crate>/tests/fixtures/`. Binary fixtures (images, blobs) are
declared in `.gitattributes` as LFS-tracked paths.

**Policy:**
- Add a fixture to LFS when a single binary file exceeds **~500 KB**, or when the cumulative
  binary fixture size in a PR exceeds **~5 MB**.
- Run `git lfs track "<pattern>"` and commit the updated `.gitattributes` in the same PR.
- Text fixtures (JSON, YAML, HTML snippets) do **not** require LFS regardless of size.

## 4. Exit Criteria for PRs

- All tests must pass.
- Code coverage must not decrease (optional but recommended).
- No new clippy warnings.
- `docs/PLAN.md` tasks must be marked as completed.

## 5. Nightly Failure Triage

`e2e.yml` runs daily at midnight UTC. When any job fails, a `notify-on-failure` job
automatically files or updates a GitHub Issue labelled **`nightly-failure`**.

**Triage steps:**

1. Open the issue linked in the notification (label: `nightly-failure`).
2. Follow the **Run** link to the failed Actions run and check which gate failed:
   - `nfr-benchmark-long` — TTFT/latency/bandwidth/capacity threshold exceeded. Check recent commits touching `core-runtime/src/sre/` or `browser.rs`.
   - `full-e2e` — `session_management`, `audit_logging`, or `spa_stable_key_stress` regressed.
   - `mcp-binary-e2e` — MCP binary smoke test broke. Likely a protocol or startup regression.
   - `evaluation-bench-full` — Feature evaluation scenario failed. Check the uploaded `evaluation-dashboard.md` artifact.
3. Reproduce locally:
   ```bash
   # NFR gates (set NFR_BENCH_MODE=full for the long variant)
   cargo test -p core-runtime --test nfr_latency --verbose
   cargo test -p core-runtime --test nfr_bandwidth --verbose
   cargo test -p core-runtime --test nfr_capacity --verbose

   # Full E2E suite
   CHROME_INSTALLED=true cargo test -p core-runtime --test session_management --verbose

   # MCP binary E2E
   CHROME_PATH=/usr/bin/chromium-browser cargo test -p mcp-server --test mcp_binary_e2e -- --ignored --nocapture

   # PR-required shipped-binary stdio smoke only
   CHROME_PATH="$(command -v google-chrome)" cargo test -p mcp-server --test mcp_binary_e2e test_mcp_binary_stdio_smoke -- --ignored --exact --nocapture
   ```
4. Fix the regression, open a PR, and close the `nightly-failure` issue once CI is green.

If the issue was a flake (not reproducible locally), add a comment explaining why and close
the issue; the next nightly run will re-open it if the problem persists.
