# Justfile for Neural-Browser Runtime

default:
    @just --list

test:
    cargo test --workspace

lint:
    cargo clippy --workspace -- -D warnings

fmt:
    cargo fmt --all

check:
    cargo check --workspace

test-all: test lint fmt
    @echo "All checks passed!"

evaluation-bench-smoke:
    rm -rf target/evaluation-bench
    mkdir -p target/evaluation-bench
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p core-runtime --test comprehensive_evaluation -- --nocapture
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p mcp-server --test comprehensive_evaluation -- --nocapture
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p skills-engine --test comprehensive_evaluation
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p plugin-host --test comprehensive_evaluation
    DRAGON_HEAD_EVAL_MODE=smoke DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p marketplace --test comprehensive_evaluation
    python3 scripts/evaluation_dashboard.py --input-dir target/evaluation-bench --output target/evaluation-dashboard.md

evaluation-bench-full:
    rm -rf target/evaluation-bench
    mkdir -p target/evaluation-bench
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p core-runtime --test comprehensive_evaluation -- --nocapture
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p mcp-server --test comprehensive_evaluation -- --nocapture
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p skills-engine --test comprehensive_evaluation
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p plugin-host --test comprehensive_evaluation
    DRAGON_HEAD_EVAL_MODE=full DRAGON_HEAD_EVAL_OUTPUT_DIR="$PWD/target/evaluation-bench" cargo test -p marketplace --test comprehensive_evaluation
    python3 scripts/evaluation_dashboard.py --input-dir target/evaluation-bench --output target/evaluation-dashboard.md
