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
