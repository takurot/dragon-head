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

---

## 4. Install script verification

- [ ] Run the install script against the new release:
  ```bash
  VERSION=vX.Y.Z bash scripts/install.sh
  dragon-head-mcp --doctor
  ```
- [ ] Confirm `--doctor` exits 0 when Chrome is present.

---

## 5. Homebrew (deferred — not yet published)

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
