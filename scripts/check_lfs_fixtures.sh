#!/usr/bin/env bash
# Verify every file matched by an LFS glob in .gitattributes is actually stored as a
# real Git LFS pointer in HEAD, not a raw blob.
#
# Background (ISSUE-251): a fixture can end up declared as `filter=lfs` in
# .gitattributes without ever being migrated (e.g. the glob was added/fixed after the
# file was first committed). Git then applies the LFS "clean" filter to the working-tree
# file for every `git status`/`git diff`, producing a small pointer-text blob that never
# matches the raw binary blob still sitting in the index — a permanent false-dirty diff
# that has nothing to do with the file's actual (correct) content.
#
# This script catches that class of bug at commit/CI time, before it reaches `main`.
#
# Usage:
#   scripts/check_lfs_fixtures.sh

set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

# Only validate files whose *resolved* filter attribute is "lfs" — this respects any
# per-path .gitattributes exceptions (e.g. `-filter`) that opt a small file back out of
# LFS tracking, rather than re-matching the same glob pattern text.
status=0
checked=0

while IFS= read -r -d '' file; do
  resolved_filter="$(git check-attr filter -- "${file}" | sed 's/.*: filter: //')"
  if [ "${resolved_filter}" != "lfs" ]; then
    continue
  fi
  checked=$((checked + 1))
  first_line="$(git cat-file -p "HEAD:${file}" 2>/dev/null | head -c 7 || true)"
  if [ "${first_line}" != "version" ]; then
    echo "error: '${file}' resolves to filter=lfs but is not a valid LFS pointer in HEAD" >&2
    echo "  fix: either 'git lfs track \"${file}\" && git add \"${file}\"' to migrate it," >&2
    echo "       or add a .gitattributes exception ('-filter -diff -merge -text') if it's under the ~500 KB LFS threshold" >&2
    status=1
  fi
done < <(git ls-files -z)

if [ "${status}" -eq 0 ]; then
  echo "ok: ${checked} LFS-tracked file(s) verified as real LFS pointers"
fi

exit "${status}"
