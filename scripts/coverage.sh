#!/usr/bin/env bash
# Run workspace code coverage locally and print a summary.
#
# Usage:
#   scripts/coverage.sh           # text summary + HTML report
#   scripts/coverage.sh --lcov    # also write lcov.info to the workspace root
#   scripts/coverage.sh --open    # open the HTML report in the browser when done
#
# Requirements: cargo-llvm-cov (installed via `cargo install cargo-llvm-cov`)
# Tip: install once with `cargo install cargo-llvm-cov --locked`
set -euo pipefail

LCOV=false
OPEN=false

for arg in "$@"; do
  case "$arg" in
    --lcov) LCOV=true ;;
    --open) OPEN=true ;;
    *) echo "Unknown argument: $arg" >&2; exit 1 ;;
  esac
done

if ! command -v cargo-llvm-cov >/dev/null 2>&1; then
  echo "cargo-llvm-cov not found. Install with:" >&2
  echo "  cargo install cargo-llvm-cov --locked" >&2
  exit 1
fi

echo "==> Collecting coverage (this rebuilds with instrumentation)..."
cargo llvm-cov nextest --workspace --no-report

echo ""
echo "==> Coverage summary:"
cargo llvm-cov report

if [ "$LCOV" = "true" ]; then
  echo ""
  echo "==> Writing lcov.info..."
  cargo llvm-cov report --lcov --output-path lcov.info
  echo "lcov.info written."
fi

echo ""
echo "==> Generating HTML report to target/llvm-cov-out/..."
cargo llvm-cov report --html --output-dir target/llvm-cov-out
echo "HTML report: target/llvm-cov-out/index.html"

if [ "$OPEN" = "true" ]; then
  if command -v open >/dev/null 2>&1; then
    open target/llvm-cov-out/index.html
  elif command -v xdg-open >/dev/null 2>&1; then
    xdg-open target/llvm-cov-out/index.html
  fi
fi
