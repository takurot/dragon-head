# Justfile for Neural-Browser Runtime

default:
    @just --list

# Requires cargo-nextest: cargo install cargo-nextest --locked
test:
    cargo nextest run --workspace

# CI profile: 3 retries with exponential backoff, fail-fast=false
test-ci:
    cargo nextest run --workspace --profile ci

lint:
    cargo clippy --workspace --all-targets -- -D warnings

fmt:
    cargo fmt --all

fmt-check:
    cargo fmt --all -- --check

check:
    cargo check --workspace

test-doc:
    cargo test --workspace --doc

deny:
    cargo deny check

test-python:
    python3 -m unittest discover -s scripts -p 'test_*.py'

test-npm-bench:
    npm --prefix bench-playwright ci
    npm --prefix bench-playwright test
    npm --prefix bench-playwright audit --audit-level=moderate

test-npm-wrapper:
    node --test npm/dragon-head-mcp/test/run.test.js

# Nonmutating source checks; browser prerequisites follow the normal test suite.
ci: fmt-check lint test-ci test-doc deny test-python test-npm-bench test-npm-wrapper

test-all: ci
    @echo "All checks passed!"

# Additional full stdio E2E suite; requires Chrome and explicitly runs ignored tests.
test-mcp-e2e:
    cargo test -p mcp-server --test mcp_binary_e2e -- --ignored --nocapture

# Real MCP binary + Chrome; local fixture and Slack API double, no live credentials.
demo-high-risk-action:
    cargo test --workspace --test mcp_high_risk_action_demo -- --ignored --nocapture

# Paired real-browser governance comparison, including failed runs and audit gaps.
bench-governance runs="20":
    #!/usr/bin/env bash
    set -euo pipefail
    cargo build --workspace --bins
    benchmark_target="${CARGO_TARGET_DIR:-target}"
    mkdir -p target/governance
    "$benchmark_target/debug/dragon-head-bench" --governance --runs "{{runs}}" --mcp-bin "$benchmark_target/debug/dragon-head-mcp" --output target/governance/report.md --output-json target/governance/report.json

evaluation-bench-smoke:
    rm -rf target/evaluation-bench
    mkdir -p target/evaluation-bench
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test --workspace --test comprehensive_evaluation -- --nocapture
    python3 scripts/evaluation_dashboard.py --input-dir target/evaluation-bench --output target/evaluation-dashboard.md

evaluation-bench-full:
    rm -rf target/evaluation-bench
    mkdir -p target/evaluation-bench
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test --workspace --test comprehensive_evaluation -- --nocapture
    python3 scripts/evaluation_dashboard.py --input-dir target/evaluation-bench --output target/evaluation-dashboard.md
