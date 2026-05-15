#!/usr/bin/env bash
# Run workspace code coverage locally and print a summary.
#
# Usage:
#   scripts/coverage.sh           # text summary only
#   scripts/coverage.sh --lcov    # also write lcov.info to the workspace root
#   scripts/coverage.sh --html    # generate HTML report at target/llvm-cov-out/
#   scripts/coverage.sh --open    # generate HTML report and open in browser
#
# Note: CI uses --profile ci (3 retries, exponential backoff). This script uses
# the default profile (2 retries). Add --profile ci if you need to reproduce
# exact CI retry behaviour.
#
# Requirements: cargo install cargo-llvm-cov --locked
set -euo pipefail

LCOV=false
HTML=false
OPEN=false

for arg in "$@"; do
  case "$arg" in
    --lcov) LCOV=true ;;
    --html) HTML=true ;;
    --open) OPEN=true; HTML=true ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

# Use cargo subcommand dispatch so any installation path (cargo install,
# taiki-e/install-action, system package) is found correctly.
if ! cargo llvm-cov --version >/dev/null 2>&1; then
  echo "cargo-llvm-cov not found. Install with:" >&2
  echo "  cargo install cargo-llvm-cov --locked" >&2
  exit 1
fi

IGNORE_REGEX='(tests/|test\.rs$)'

echo "==> Collecting coverage (rebuilds workspace with instrumentation)..."
cargo llvm-cov nextest \
  --workspace \
  --no-report \
  --ignore-filename-regex "$IGNORE_REGEX"

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
