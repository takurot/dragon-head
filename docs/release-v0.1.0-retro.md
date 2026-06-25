# Release retro: v0.1.0 (first dragon-head-mcp release)

Date: 2026-06-25
Tag: `v0.1.0`
Release: https://github.com/takurot/dragon-head/releases/tag/v0.1.0

This was the first time `.github/workflows/release.yml` was ever triggered.
The pipeline and `docs/release-checklist.md` were already correct and
complete, but executing them for real surfaced one infra issue worth
recording. This doc captures what actually happened, for future releases.

## What happened

1. **Pre-release verification on `main`** — `cargo test --workspace`,
   `cargo fmt --all -- --check`, `cargo clippy --workspace -- -D warnings`,
   a release build, and `--doctor` / `--init` smoke tests all passed before
   tagging. No version bump was needed: `mcp-server/Cargo.toml` was already
   at `0.1.0` and this was the first release.
2. **README date bump** — `README.md` "Last updated" bumped to the release
   date, committed on `main`.
3. **Tag and push** — `git tag v0.1.0 && git push origin v0.1.0`, which
   triggered `release.yml`.
4. **Unrelated CI flake on `main`** — the regular `CI` workflow's `test` job
   (triggered by the same push to `main`, separate from `release.yml`)
   crashed with `System.IO.IOException: No space left on device` — a
   GitHub-hosted runner disk-space exhaustion, not a real test failure.
   Fixed by `gh run rerun --failed`; the rerun passed cleanly. Not specific
   to this release, just a transient hosted-runner issue.
5. **`macos-13` runner never picked up the job** — the `x86_64-apple-darwin`
   build job (`runs-on: macos-13`) stayed in `queued` state indefinitely,
   `runner_id: null`, even after cancel + rerun. All other 4 platform jobs
   (`macos-latest`, `ubuntu-latest` x2, `windows-latest`) started normally
   within seconds. This strongly indicates GitHub had already retired/
   stopped scheduling the `macos-13` hosted-runner image — it wasn't a queue
   backlog, since rerunning twice produced the identical symptom.
6. **Fix and re-tag** — changed `release.yml`'s `x86_64-apple-darwin` job to
   `runs-on: macos-latest`. Rust cross-compiles `x86_64-apple-darwin` from an
   Apple Silicon (`aarch64`) runner without issue via
   `rustup target add x86_64-apple-darwin` + `cargo build --target ...` —
   no actual Intel hardware is required, since this is a build-time target,
   not an execution-time one. Committed the fix to `main`, then **deleted
   and recreated the `v0.1.0` tag** at the new `main` HEAD (no GitHub Release
   had been published yet at this point, since the final `release` job had
   never run — only the build matrix was stuck) and pushed it again. All 5
   platform builds plus the final `release` job succeeded on this second
   pass.
7. **Post-release verification** — downloaded the macOS arm64 artifact +
   `.sha256`, verified the checksum, ran `--doctor` and `--init`, and ran
   `scripts/install.sh` end-to-end against the real `v0.1.0` release
   (installed to a scratch `INSTALL_DIR` rather than `/usr/local/bin` to
   avoid touching the local machine's system path during verification).

## Lessons for future releases

- **`docs/release-checklist.md` is accurate and sufficient** — no changes
  needed to the checklist itself; this doc is a supplement, not a
  replacement.
- **Watch for indefinitely `queued` jobs**, not just failures. A job that
  fails fast is easy to triage; a job stuck `queued` with no runner assigned
  for tens of minutes — especially if a rerun reproduces the exact same
  symptom — is a sign the `runs-on` label itself may no longer be served by
  GitHub, not a transient capacity issue. Check
  `gh api repos/<owner>/<repo>/actions/jobs/<job-id>` for `runner_id` staying
  `null`/`0` as the diagnostic signal.
- **Tag deletion/recreation is safe pre-publish.** Since the `release` job
  (which publishes the GitHub Release) only runs after the full build matrix
  succeeds, a stuck build job means nothing has been published yet — deleting
  and recreating the tag at a fixed commit is a clean way to retry the whole
  pipeline from scratch without any rollback of public artifacts.
- **A CI flake on `main` and a release-pipeline problem can coincide and are
  independent** — don't assume they share a root cause. The disk-space crash
  was pure hosted-runner noise; the `macos-13` issue was a real, permanent
  infra change. Diagnose each on its own evidence.
- `release.yml` now builds `x86_64-apple-darwin` on `macos-latest` via
  cross-compilation; if GitHub ever fully drops Intel macOS runner labels
  this is also the long-term-safe choice, since it doesn't depend on
  GitHub continuing to host any Intel Mac hardware at all.

See `docs/release-checklist.md` for the step-by-step checklist this retro
supplements.
