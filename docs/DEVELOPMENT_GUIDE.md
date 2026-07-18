# DEVELOPMENT_GUIDE.md

すべてのコマンドは実際に存在するファイル(`Justfile`, `Cargo.toml`,
`.github/workflows/*.yml`, `rust-toolchain.toml`)から確認済み。未確認の項目は
明記する。

## セットアップ

- Rustツールチェイン: `rust-toolchain.toml` で `channel = "stable"` を指定。
  `rustup` がインストール済みなら自動でstableが使われる。
- Chrome/Chromium: 多くの統合テストとMCPサーバーの実行に必要。`CHROME_PATH`
  で明示的に指定できる。
- 開発コンテナ: `.devcontainer/Dockerfile` にChromeプリインストール済みの
  Dockerイメージ定義がある(VS Code Dev Containers向け)。

```bash
git clone https://github.com/takurot/dragon-head.git
cd dragon-head
cargo build
```

## 依存関係のインストール

Cargoが`Cargo.toml`/`Cargo.lock`から自動解決する。手動インストール手順は不要。
ワークスペース共通バージョンは `[workspace.dependencies]`(root `Cargo.toml`)
で固定: `tokio 1.36`, `anyhow 1.0`, `thiserror 1.0`, `tracing 0.1`,
`tracing-subscriber 0.3`, `headless_chrome 1.0`, `serde 1.0`, `serde_json 1.0`,
`toml 0.8`。

## ローカル実行

```bash
# MCPサーバーのChrome検出確認
cargo run -p mcp-server --bin dragon-head-mcp -- --doctor

# MCPクライアント用設定スニペットを出力
cargo run -p mcp-server --bin dragon-head-mcp -- --init claude-code

# Chrome不要の開発用サンプル
cargo run --example quickstart
cargo run --example policy_cookbook
```

`dragon-head-mcp` は標準入力からJSON-RPCを受け取る前提のため、ターミナルで
直接実行すると即終了する(MCPクライアント経由の起動を想定)。

## テスト実行

```bash
just test            # cargo nextest run --workspace
just test-ci          # cargo nextest run --workspace --profile ci (CIと同等)
cargo test -p <crate> --test <file>   # 個別integrationテスト
cargo test --workspace --doc          # doctest
```

ブラウザ依存テストは `test_bench_support::should_skip_browser_tests()` で
Chrome未検出時に自動スキップされる。明示的に有効化するには:

```bash
CHROME_INSTALLED=true cargo test -p core-runtime --test semantic_wait
```

nextestのプロファイルは `.config/nextest.toml` に `default`/`ci` の2種類。
**`ci` プロファイルは `default` を継承しないため全フィールドを repeat する
必要がある**。

## lint / format / typecheck

```bash
just fmt    # cargo fmt --all
just lint   # cargo clippy --workspace -- -D warnings
just check  # cargo check --workspace
```

`.rustfmt.toml`/`clippy.toml` は存在しない(デフォルト設定を使用)。
warningはCIで `-D warnings` によりエラー扱い。

## ビルド

```bash
cargo build
cargo build -p mcp-server --bin dragon-head-mcp --release
```

リリースバイナリは `.github/workflows/release.yml` がタグpush(`v*`)時に
macOS arm64/x64、Linux x64/arm64、Windows x64 向けにビルドし、sha256
チェックサム付きで GitHub Releases にアップロードする。

## 環境変数

| 変数 | 用途 | 既定値 |
|---|---|---|
| `CHROME_PATH` | Chrome/Chromiumバイナリパスの上書き | 自動検出 |
| `PROMPT_INJECTION_MODE` | `prompt_injection.mode` の上書き(`off`/`report_only`/`redact`) | `report_only` |
| `PROMPT_INJECTION_ADDITIONAL_PHRASES` | 追加検知phraseをJSON文字列配列で上書き(`[]`で明示的に空) | `config.toml`の値 |
| `POLICY_FILE` | `policy.file` の上書き | 未設定 |
| `AUDIT_LOG_DIR` | 監査ログの永続化先ディレクトリ | 未設定(永続化なし) |
| `AUDIT_LOG_MAX_BYTES` | ログローテーション閾値(バイト) | 10485760 (10MiB) |
| `AUDIT_DURABILITY` | `flush` または `sync` | `flush` |
| `AUDIT_LOG_STDOUT` | 設定されていれば(値は任意)監査ログを標準エラー出力にも出力(互換性のため名称を維持) | 未設定(出力なし) |
| `CHROME_INSTALLED` | CIでChrome利用可能を示すフラグ(テストゲート用) | 未設定 |
| `CI` | 汎用CI検出 | 未設定 |

`config.toml`

環境変数は常に `config.toml` の対応するキーより優先される
(`mcp-server/src/config.rs`)。

## よくあるエラーと対処

- **`dragon-head-mcp` をターミナルで直接実行すると即終了する** —
  仕様通り。MCPクライアント経由で起動すること。
- **Chrome未検出でテストが失敗/スキップされる** — `--doctor` で検出状況を
  確認し、`CHROME_PATH` を設定する。
- **`cargo clippy` がwarningで失敗する** — `-D warnings` によりCIではエラー
  扱い。ローカルでも `just lint` で同条件を再現できる。
- ビルドエラーの解析手順は別途確立されていない(プロジェクト固有のFAQは
  「未確認」)。

## CI/CD概要

`.github/workflows/`:

- **`ci.yml`** — `main`/`feature/**`/`codex/**` へのpush、`main` へのPRで
  起動。`lint`, `security-audit`(rustsec/audit-check, informational),
  `cargo-deny`(`deny.toml` による強制ゲート), `shellcheck`, `test`
  (nextest --profile ci + doctest), `coverage`(cargo-llvm-cov, 行カバレッジ
  70%未満で失敗), 各種regression/compliance/schemaジョブ
  (`sre-regression`, `policy-regression`, `mcp-protocol-compliance` 等),
  `nfr-benchmark-short`, PR限定の `evaluation-bench-smoke`。nextestは
  `nextest@0.9.133` 固定。
- **`e2e.yml`**(Nightly) — 日次cron + 手動実行。`nfr-benchmark-long`,
  `full-e2e`, `mcp-binary-e2e`, `evaluation-bench-full`。
- **`release.yml`** — タグ `v*` push時にクロスビルド+チェックサム生成+
  GitHub Release作成。

## コードスタイル

- Rust標準の `snake_case`/`PascalCase`/`SCREAMING_SNAKE_CASE` 命名。
- `cargo fmt`(デフォルト設定)+ `cargo clippy -D warnings` を必須。
- ライブラリ層は `thiserror`、アプリ層は `anyhow` でエラー型を扱う
  (ユーザー個人ルール `~/.claude/rules/rust/coding-style.md` 準拠)。
- Rust固有の落とし穴は、追跡済みドキュメントでは `docs/AI_CONTEXT.md` と
  `docs/AI_TASK_GUIDE.md` の高リスク領域メモを正本として参照する。ローカルの
  `AGENTS.md` / `CLAUDE.md` が存在する場合は補助指示として扱う。

## ブランチ・コミット方針

- 観測されたブランチ命名: `main`, `feature/**`, `codex/**`(CIトリガー設定
  `ci.yml` より)。明文化されたブランチ規約ドキュメントは見つからず、
  **未確認**。
- コミットメッセージの規約ファイル(`CONTRIBUTING.md` 等)はリポジトリ内に
  見つからず、**未確認**。直近のコミット例(`git log`)は
  `[ISSUE-NNN] <要約> (#PR番号)` 形式が多く使われている。
- `docs/PROMPT.md` にPR実装ワークフロー(計画→TDD→QAゲート→PR作成→レビュー
  →マージ)の手順が定義されている。大きな変更を行う際は参照すること。
