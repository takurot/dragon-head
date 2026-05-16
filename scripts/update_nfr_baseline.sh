#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASELINE_DIR="${REPO_ROOT}/nfr-baseline"
METRICS_DIR="${REPO_ROOT}/core-runtime/target/nfr-metrics"

usage() {
    echo "Usage: $(basename "$0") [--metrics-dir <path>]"
    echo ""
    echo "Copies NFR metric JSON files from metrics-dir into nfr-baseline/."
    echo ""
    echo "Options:"
    echo "  --metrics-dir <path>  Source directory for metric files"
    echo "                        (default: core-runtime/target/nfr-metrics)"
    exit 1
}

while [[ $# -gt 0 ]]; do
    case "$1" in
        --metrics-dir)
            if [[ -z "${2:-}" ]]; then
                echo "error: --metrics-dir requires a path argument" >&2
                usage
            fi
            METRICS_DIR="$2"
            shift 2
            ;;
        -h|--help)
            usage
            ;;
        *)
            echo "error: unknown argument: $1" >&2
            usage
            ;;
    esac
done

if [[ ! -d "${METRICS_DIR}" ]]; then
    echo "error: metrics directory does not exist: ${METRICS_DIR}" >&2
    exit 1
fi

mapfile -t json_files < <(find "${METRICS_DIR}" -maxdepth 1 -name "*.json" | sort)

if [[ "${#json_files[@]}" -eq 0 ]]; then
    echo "error: no .json files found in ${METRICS_DIR}" >&2
    exit 1
fi

mkdir -p "${BASELINE_DIR}"

for src in "${json_files[@]}"; do
    filename="$(basename "${src}")"
    dest="${BASELINE_DIR}/${filename}"
    cp "${src}" "${dest}"
    echo "updated: nfr-baseline/${filename}"
done

echo ""
echo "Baseline updated with ${#json_files[@]} file(s) from ${METRICS_DIR}"
