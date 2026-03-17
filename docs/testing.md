# Testing Strategy

This document outlines the testing strategy for the Neural-Browser Runtime project.

## 1. Test Levels

We follow the "Testing Pyramid" approach with the following layers:

### 1.1 Unit Tests (Level 1)
- **Scope**: Individual functions, structs, and modules.
- **Tools**: Rust intrinsic `#[test]`, `pytest` (for Python bindings).
- **Location**: Co-located with code in `src/` or `tests/unit/`.
- **Frequency**: Run on every file save (local), check on every commit (CI).
- **Coverage Target**: High (>80%).

### 1.2 Integration Tests (Level 2)
- **Scope**: Interaction between modules (e.g., SRE + CDP client).
- **Tools**: Rust `tests/` directory.
- **Location**: `tests/integration/`.
- **Frequency**: Run on every PR.
- **Mocking**: External services (CDP) may be mocked or run against a real headless browser docker container.

### 1.3 E2E Tests (Level 3)
- **Scope**: Full system verification (Client -> Runtime -> Browser -> Website).
- **Tools**: Custom runner, real Chromium instance.
- **Location**: `tests/e2e/`.
- **Frequency**: Run on PR (Smoke suite), Nightly (Full suite).

## 2. CI/CD Pipeline

The CI pipeline is defined in `.github/workflows/`.

- **`ci.yml`**: Runs `fmt`, `clippy`, unit tests, and integration tests.
- **`e2e.yml`**: Runs E2E tests against a headless browser.
- **Performance Gate (PR Short)**: `ci.yml` runs a short NFR suite (`ttft`, `nfr_latency`, `nfr_bandwidth`, `nfr_capacity`) and generates `core-runtime/target/nfr-dashboard.md`.
- **Performance Gate (Nightly Full)**: `e2e.yml` runs the full NFR suite (including long TTFT and full capacity targets) and publishes the same dashboard format for regression tracking.
- **Threshold Enforcement**: `scripts/nfr_dashboard.py` evaluates metric thresholds from JSON outputs and fails CI on regressions.
- **Comprehensive Evaluation Bench**: `just evaluation-bench-smoke` runs crate-spanning feature evaluation suites and emits JSON reports plus `target/evaluation-dashboard.md`. Nightly/full runs use the same format with `DRAGON_HEAD_EVAL_MODE=full`.

## 2.1 Comprehensive Evaluation Bench

- **Goal**: Provide a single dashboard that verifies the main Dragon Head feature areas across `core-runtime`, `mcp-server`, `skills-engine`, `plugin-host`, and `marketplace`.
- **Modes**:
  - `smoke`: Required on PRs. Covers representative scenarios for state capture, action recovery, wait semantics, policy/HITL, audit/session, MCP flow, skill execution, plugin validation, and marketplace accounting.
  - `full`: Runs on nightly/manual workflows. Uses the same report format and is reserved for expanded scenario sets and longer-running variants.
- **Artifacts**:
  - JSON reports: `target/evaluation-bench/*.json`
  - Markdown dashboard: `target/evaluation-dashboard.md`
- **Registration Rule**: New major features are not considered complete until a corresponding scenario is added to the evaluation bench or an explicit exemption is documented in `docs/`.

## 3. Running Tests Locally

```bash
# Run all unit and integration tests
cargo test

# Run specific test
cargo test test_name

# Run E2E tests (requires setup)
cargo test --test e2e
```

## 4. Exit Criteria for PRs

- All tests must pass.
- Code coverage must not decrease (optional but recommended).
- No new clippy warnings.
- `docs/PLAN.md` tasks must be marked as completed.
