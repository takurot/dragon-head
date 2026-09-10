#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
BASELINE_DIR="${REPO_ROOT}/nfr-baseline"
METRICS_DIR="${REPO_ROOT}/core-runtime/target/nfr-metrics"
FORCE=false

usage() {
    echo "Usage: $(basename "$0") [--metrics-dir <path>] [--force]"
    echo ""
    echo "Copies NFR metric JSON files from metrics-dir into nfr-baseline/."
    echo ""
    echo "Options:"
    echo "  --metrics-dir <path>  Source directory for metric files"
    echo "                        (default: core-runtime/target/nfr-metrics)"
    echo "  --force               Actually overwrite existing baseline files that"
    echo "                        differ from the new metrics. Without this flag,"
    echo "                        differing files are reported but not written —"
    echo "                        only brand-new baseline files are copied."
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
        --force)
            FORCE=true
            shift
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

# Pass 1: validate every source file is well-formed JSON *before* touching
# the baseline directory at all — a corrupt/truncated metrics file (e.g.
# from a crashed benchmark run) must never make it into a committed
# baseline (issue #285).
for src in "${json_files[@]}"; do
    if ! jq empty "${src}" >/dev/null 2>&1; then
        echo "error: '${src}' is not valid JSON — aborting before writing anything to ${BASELINE_DIR}" >&2
        exit 1
    fi
done

# Pass 2: classify each file (new / unchanged / changed) using the
# validated sources, without writing anything yet, so a would-overwrite
# file can be reported and gated behind --force (issue #285).
declare -a to_write=()
declare -a skipped_without_force=()
new_count=0
unchanged_count=0

for src in "${json_files[@]}"; do
    filename="$(basename "${src}")"
    dest="${BASELINE_DIR}/${filename}"

    if [[ ! -e "${dest}" ]]; then
        to_write+=("${src}")
        new_count=$((new_count + 1))
        continue
    fi

    if cmp -s "${src}" "${dest}"; then
        unchanged_count=$((unchanged_count + 1))
        continue
    fi

    if [[ "${FORCE}" == "true" ]]; then
        to_write+=("${src}")
    else
        echo "would overwrite: nfr-baseline/${filename} (differs from new metrics — rerun with --force to apply)" >&2
        skipped_without_force+=("${filename}")
    fi
done

# Clean up a same-directory temp file left behind if `cp`/`mv` below fails
# partway through the batch (issue #285 follow-up: nothing corrupts the
# already-written baseline files, but a stray *.tmp.$$ would otherwise sit
# around until manually noticed).
trap 'rm -f "${BASELINE_DIR}"/*.tmp.$$' EXIT

# Pass 3: write every approved file (new files, plus changed ones when
# --force was given) to a same-directory temp path first, then `mv` each
# into place. `mv` within one filesystem is atomic, so even if a later file
# in the batch fails to copy, every file written so far is either fully the
# old baseline or fully the new one — never a partially written/truncated
# file (issue #285: "batch update non-atomic"). New files are always safe
# to write (there's nothing to overwrite), so they're applied even when
# some other file was skipped for lack of --force.
written_count=0
for src in "${to_write[@]}"; do
    filename="$(basename "${src}")"
    dest="${BASELINE_DIR}/${filename}"
    tmp_dest="${dest}.tmp.$$"
    cp "${src}" "${tmp_dest}"
    mv "${tmp_dest}" "${dest}"
    echo "updated: nfr-baseline/${filename}"
    written_count=$((written_count + 1))
done

echo ""
echo "Baseline: ${written_count} file(s) written (${new_count} new), ${unchanged_count} unchanged, ${#skipped_without_force[@]} skipped (needs --force), from ${METRICS_DIR}"

# Exit non-zero whenever anything was left un-applied — a caller that only
# checks the exit code (a CI step, a follow-up `git add nfr-baseline/`)
# must be able to tell a partial run from a fully-applied one, even when
# some brand-new files were written successfully alongside the skips
# (issue #285 follow-up).
if [[ "${#skipped_without_force[@]}" -gt 0 ]]; then
    echo "Review the 'would overwrite' warnings above, then rerun with --force to apply them." >&2
    exit 1
fi
