# Release Checklist — dragon-head-mcp

Follow this checklist for every versioned release of `dragon-head-mcp`.

---

## 1. Pre-release

- [ ] Decide the version number (follow [SemVer](https://semver.org/)):
  - **MAJOR** — breaking MCP protocol or config changes
  - **MINOR** — new tools, new `--init` client targets, new `--doctor` checks
  - **PATCH** — bug fixes, doc updates, security patches
- [ ] Update `version` in the workspace root `Cargo.toml` and in `mcp-server/Cargo.toml`.
- [ ] Update `README.md` — bump the **Last updated** date at the top.
- [ ] Run the full test suite and confirm all pass:
  ```bash
  cargo test --workspace
  cargo fmt --all -- --check
  cargo clippy --workspace -- -D warnings
  ```
- [ ] Smoke-test the binary locally:
  ```bash
  cargo build -p mcp-server --bin dragon-head-mcp --release
  ./target/release/dragon-head-mcp --doctor
  ./target/release/dragon-head-mcp --init
  ./target/release/dragon-head-mcp --init claude-desktop
  ```

---

## 2. Tag and release

- [ ] Commit the version bump:
  ```bash
  git add Cargo.toml Cargo.lock mcp-server/Cargo.toml README.md
  git commit -m "chore(release): bump version to vX.Y.Z"
  ```
- [ ] Create and push the release tag:
  ```bash
  git tag vX.Y.Z
  git push origin vX.Y.Z
  ```
- [ ] Verify the **Release** GitHub Actions workflow (`release.yml`) triggers on the tag push.
- [ ] Wait for all matrix jobs to finish (macOS arm64/x64, Linux x64/arm64, Windows x64).
- [ ] Confirm the GitHub Release is created with `generate_release_notes: true`.

---

## 3. Post-release verification

For each platform artifact:

- [ ] Download the binary and its `.sha256` file from the GitHub Release page.
- [ ] Verify the checksum:
  ```bash
  # macOS
  shasum -a 256 -c dragon-head-mcp-macos-arm64.sha256
  # Linux
  sha256sum -c dragon-head-mcp-linux-x64.sha256
  ```
- [ ] Make executable and run `--doctor`:
  ```bash
  chmod +x dragon-head-mcp-macos-arm64
  ./dragon-head-mcp-macos-arm64 --doctor
  ```
- [ ] Run `--init` on at least one platform:
  ```bash
  ./dragon-head-mcp-macos-arm64 --init
  ./dragon-head-mcp-macos-arm64 --init claude-desktop
  ```

Artifacts to verify:

| Artifact | Platform |
|---|---|
| `dragon-head-mcp-macos-arm64` | macOS Apple Silicon |
| `dragon-head-mcp-macos-x64` | macOS Intel |
| `dragon-head-mcp-linux-x64` | Linux x86-64 |
| `dragon-head-mcp-linux-arm64` | Linux arm64 |
| `dragon-head-mcp-windows-x64.exe` | Windows x86-64 |
| `dragon-head-mcp-windows-arm64.exe` | Windows ARM64 |

---

## 4. Install script verification

- [ ] Run the install script against the new release:
  ```bash
  VERSION=vX.Y.Z bash scripts/install.sh
  dragon-head-mcp --doctor
  ```
- [ ] Confirm `--doctor` exits 0 when Chrome is present.

---

## 5. npm verification

> **New platform package? Read this first.** `release.yml`'s npm jobs publish via
> **OIDC trusted publishing** (`id-token: write`, no long-lived `NPM_TOKEN`). Trusted
> publishing is configured *per package* on npmjs.com, and that configuration screen
> only exists once the package has been published at least once — so a package name
> that has **never been published before** (e.g. a newly added platform target) will
> fail CI with `npm error code ENEEDAUTH` on every attempt, because there's nothing
> for the OIDC token to be trusted against yet. This is not a transient/flaky failure;
> retrying the job will not help until the one-time manual step below is done.
>
> **One-time fix, before tagging a release that includes a brand-new platform package:**
> 1. Publish it once yourself, from an authenticated (2FA-verified) local npm session:
>    ```bash
>    cd npm/platform/<new-platform>
>    npm login   # if not already logged in
>    npm publish --access public
>    ```
> 2. On npmjs.com, open that package → **Settings → Trusted Publisher → Add** and fill in:
>    | Field | Value |
>    |---|---|
>    | Organization or user | `takurot` |
>    | Repository | `dragon-head` |
>    | Workflow filename | `release.yml` |
>    | Environment name | *(leave blank — this workflow doesn't use `environment:`)* |
>    | Label | optional, e.g. `release.yml (GitHub Actions)` |
>    | "Allow npm publish" checkbox | **check it** — unchecked, OIDC auth still succeeds but the actual publish is rejected |
>
>    Sanity-check the values against an existing package's Trusted Publisher settings
>    (e.g. `dragon-head-mcp-linux-x64`) if unsure.
> 3. After that, CI publishes for that package work automatically like the others.
>
> If a release run fails with `ENEEDAUTH` for one package after the others already
> published successfully, this is almost certainly the cause — do the one-time setup
> above, then re-run just the failed job (`gh run rerun <run-id> --failed`); the
> publish script's idempotent "already published, skipping" check means already-published
> packages won't be touched again.
>
> **After a provenance-signed publish succeeds**, npm can take a few minutes before the
> new version is queryable (`npm view <pkg>@<version>` may 404 briefly) — this is normal
> registry propagation, not a failure. Poll rather than assume it failed:
> ```bash
> until npm view <pkg>@<version> version >/dev/null 2>&1; do sleep 20; done
> ```

- [ ] Confirm the `npm-publish-platforms` and `publish-npm-wrapper` CI jobs completed green.
- [ ] Verify the wrapper package is live and at the correct version:
  ```bash
  npm view dragon-head-mcp versions --json
  ```
- [ ] Smoke-test the wrapper install in a temporary directory:
  ```bash
  cd "$(mktemp -d)"
  npm install dragon-head-mcp@X.Y.Z
  ./node_modules/.bin/dragon-head-mcp --doctor
  ```
- [ ] Verify each platform package is also live:
  ```bash
  for pkg in dragon-head-mcp-darwin-arm64 dragon-head-mcp-darwin-x64 \
              dragon-head-mcp-linux-x64 dragon-head-mcp-linux-arm64 \
              dragon-head-mcp-win32-x64 dragon-head-mcp-win32-arm64; do
    npm view "$pkg" version
  done
  ```
- [ ] Test global install (on at least one platform):
  ```bash
  npx dragon-head-mcp@X.Y.Z --doctor
  ```

---

## 6. Homebrew (deferred — not yet published)

> **Status:** Planned. A `takurot/tap` formula is tracked separately.
>
> When the tap is live, add steps here:
> - `brew upgrade takurot/tap/dragon-head`
> - `dragon-head-mcp --doctor`

---

## 6. Announce and close

- [ ] Close the corresponding GitHub Issue (if any) with a comment linking the release.
- [ ] Post a release announcement if applicable (project blog, Discord, etc.).

---

## Rollback

If a release is broken after publishing:

1. Delete the GitHub Release (draft or published) from the GitHub UI.
2. Delete the tag:
   ```bash
   git tag -d vX.Y.Z
   git push origin :refs/tags/vX.Y.Z
   ```
3. Fix the issue, re-test, and re-tag.
