#!/usr/bin/env bash
# Run workspace code coverage locally and print a summary.
#
# Usage:
#   scripts/coverage.sh                  # text summary only
#   scripts/coverage.sh --lcov           # also write lcov.info to the workspace root
#   scripts/coverage.sh --html           # generate HTML report at target/llvm-cov-out/
#   scripts/coverage.sh --open           # generate HTML report and open in browser
#   scripts/coverage.sh --profile ci     # reproduce CI's nextest retry profile
#
# Note: CI uses --profile ci (3 retries, exponential backoff). This script uses
# the default profile (2 retries) unless --profile is passed.
#
# Requirements: cargo install cargo-llvm-cov --locked, cargo install cargo-nextest --locked
set -euo pipefail

# Run from the workspace root regardless of the caller's cwd, so `lcov.info`
# and the HTML report land in the documented location (issue #285) instead
# of wherever the script happened to be invoked from.
cd "$(git rev-parse --show-toplevel)"

LCOV=false
HTML=false
OPEN=false
PROFILE=""

while [ $# -gt 0 ]; do
  case "$1" in
    --lcov) LCOV=true; shift ;;
    --html) HTML=true; shift ;;
    --open) OPEN=true; HTML=true; shift ;;
    --profile)
      if [ $# -lt 2 ]; then
        echo "--profile requires a value (e.g. --profile ci)" >&2
        exit 1
      fi
      PROFILE="$2"
      shift 2
      ;;
    *) echo "Unknown argument: $1" >&2; exit 1 ;;
  esac
done

# Use cargo subcommand dispatch so any installation path (cargo install,
# taiki-e/install-action, system package) is found correctly.
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov not found. Install with:" >&2
  echo "  cargo install cargo-llvm-cov --locked" >&2
  exit 1
fi
if ! cargo nextest --version >/dev/null 2>&1; then
  echo "cargo-nextest not found (required by 'cargo llvm-cov nextest'). Install with:" >&2
  echo "  cargo install cargo-nextest --locked" >&2
  exit 1
fi

IGNORE_REGEX='(tests/|test\.rs$)'

PROFILE_ARGS=()
if [ -n "$PROFILE" ]; then
  # `--profile` here is nextest's own flag (cargo llvm-cov passes anything
  # after its own [OPTIONS] straight through to `cargo nextest run`), not a
  # cargo-llvm-cov flag — verified via `cargo nextest run --help`.
  PROFILE_ARGS=(--profile "$PROFILE")
fi

echo "==> Collecting coverage (rebuilds workspace with instrumentation)..."
cargo llvm-cov nextest \
  --workspace \
  --no-report \
  --ignore-filename-regex "$IGNORE_REGEX" \
  "${PROFILE_ARGS[@]}"

echo ""
echo "==> Coverage summary:"
# --text is required; bare 'cargo llvm-cov report' emits no stdout.
cargo llvm-cov report \
  --text \
  --ignore-filename-regex "$IGNORE_REGEX"

if [ "$LCOV" = "true" ]; then
  echo ""
  echo "==> Writing lcov.info..."
  cargo llvm-cov report \
    --lcov \
    --output-path lcov.info \
    --ignore-filename-regex "$IGNORE_REGEX"
  echo "lcov.info written."
fi

if [ "$HTML" = "true" ]; then
  echo ""
  echo "==> Generating HTML report to target/llvm-cov-out/..."
  cargo llvm-cov report \
    --html \
    --output-dir target/llvm-cov-out \
    --ignore-filename-regex "$IGNORE_REGEX"
  echo "HTML report: target/llvm-cov-out/index.html"

  if [ "$OPEN" = "true" ]; then
    if command -v open >/dev/null 2>&1; then
      open target/llvm-cov-out/index.html
    elif command -v xdg-open >/dev/null 2>&1; then
      xdg-open target/llvm-cov-out/index.html
    fi
  fi
fi
