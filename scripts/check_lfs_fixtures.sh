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
  # `-z` gives unambiguous NUL-separated `path\0attr\0value\0` triples,
  # instead of parsing the human-readable "path: attr: value" text with a
  # `sed` pattern that could misparse a path containing the literal
  # substring ": filter: " (issue #285).
  resolved_filter="$(git check-attr -z filter -- "${file}" | tr '\0' '\n' | tail -n 1)"
  if [ "${resolved_filter}" != "lfs" ]; then
    continue
  fi
  checked=$((checked + 1))

  # Prefer the committed HEAD blob, but fall back to the index blob for a
  # file that's staged/tracked but not yet in any commit (e.g. a fixture
  # added in the change currently being checked) — previously `git cat-file
  # -p HEAD:...` failing on a brand-new file was silently swallowed by
  # `2>/dev/null || true`, leaving `first_line` empty and falsely flagging
  # every new LFS fixture as an invalid pointer (issue #285).
  #
  # Pipe straight through `head -n 2` rather than capturing the whole blob
  # into a shell variable first — an LFS pointer is a few dozen bytes, but a
  # *mis-tracked raw binary* (exactly the failure case this script exists to
  # catch) can be megabytes, and we only ever need its first two lines.
  #
  # `|| true` on each pipeline is required, not cosmetic: with `pipefail`
  # active, `head -n 2` closing its read end early (as it does on a
  # multi-megabyte binary — precisely the adversarial input this script
  # targets) sends `git cat-file -p` a SIGPIPE, so the pipeline's exit
  # status is `cat-file`'s 141, not `head`'s 0. Without `|| true`, `set -e`
  # aborts the whole script right here with no diagnostic — before the
  # file-specific error message a few lines below ever gets a chance to
  # print. `git cat-file -e` above already confirmed the object exists, so
  # this can't silently mask a real "object not found" error the way the
  # old code's blanket `2>/dev/null || true` did (issue #285 follow-up).
  if git cat-file -e "HEAD:${file}" 2>/dev/null; then
    header="$(git cat-file -p "HEAD:${file}" | head -n 2)" || true
  elif git cat-file -e ":${file}" 2>/dev/null; then
    header="$(git cat-file -p ":${file}" | head -n 2)" || true
  else
    echo "error: '${file}' resolves to filter=lfs but has no readable blob in HEAD or the index" >&2
    status=1
    continue
  fi

  # A real LFS pointer's first two lines are a fixed, versioned spec
  # header followed by the OID line — checking only the first 7 bytes
  # ("version") would also match any ordinary text file that happens to
  # start with that word (e.g. a changelog beginning "version 1.0...")
  # (issue #285).
  first_line="$(printf '%s\n' "${header}" | sed -n '1p')"
  second_line="$(printf '%s\n' "${header}" | sed -n '2p')"
  case "${first_line}" in
    "version https://git-lfs.github.com/spec/v"*) ;;
    *)
      echo "error: '${file}' resolves to filter=lfs but is not a valid LFS pointer" >&2
      echo "  fix: either 'git lfs track \"${file}\" && git add \"${file}\"' to migrate it," >&2
      echo "       or add a .gitattributes exception ('-filter -diff -merge -text') if it's under the ~500 KB LFS threshold" >&2
      status=1
      continue
      ;;
  esac
  case "${second_line}" in
    "oid sha256:"*) ;;
    *)
      echo "error: '${file}' has an LFS-pointer-shaped first line but no valid 'oid sha256:' line" >&2
      status=1
      ;;
  esac
done < <(git ls-files -z)

if [ "${status}" -eq 0 ]; then
  echo "ok: ${checked} LFS-tracked file(s) verified as real LFS pointers"
fi

exit "${status}"
