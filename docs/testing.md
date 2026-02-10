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
