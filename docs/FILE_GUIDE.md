# FILE_GUIDE.md

## Directory overview

| パス | 役割 | 変更頻度 | 注意点 |
|---|---|---:|---|
| `core-runtime/src/` | Chrome/CDP session, Semantic State, policy, audit, privacy, plugins, speculative state | 高 | `stable_key.rs`/`browser.rs` は影響範囲が大きい |
| `core-runtime/tests/` | core-runtime の統合テスト(35ファイル) | 高 | 機能変更時は対応するテストファイルを必ず確認 |
| `mcp-server/src/` | `dragon-head-mcp` バイナリ、MCPツールディスパッチ、設定読み込み | 高 | `lib.rs` が巨大、ツール追加はここに集約 |
| `mcp-server/tests/` | MCPプロトコル/契約テスト(12ファイル) | 中 | ツール追加時はフィクスチャ更新が必要 |
| `skills-engine/src/` | 宣言的スキル(ワークフロー)定義と実行 | 低〜中 | `lib.rs` 単一ファイル |
| `plugin-host/src/` | Wasmプラグインの検証・実行 | 低 | 署名/SBOM検証ロジックは慎重に |
| `marketplace/src/` | プラグイン/ドメインパックのメタデータ | 低 | |
| `hitl-bridge/src/` | Slack/Teams HITLブリッジ(独立バイナリ) | 低 | `lock.rs`/`server.rs` のHMAC検証は変更注意 |
| `bench/src/` | NFR/ROIベンチマークハーネス | 低 | |
| `test-bench-support/src/` | テスト用Chrome検出ヘルパー | 低 | |
| `examples/` | Chrome不要のサンプル+MCPリクエスト/レスポンスJSON | 低 | READMEの説明と同期させる |
| `docs/` | 仕様・計画・本ドキュメント群 | 中 | `PLAN.md` はPRごとに更新 |
| `scripts/` | インストーラ/ベンチ集計/NFRトレンド等のPythonスクリプト | 低 | |
| `nfr-baseline/*.json` | NFR回帰検出の基準値 | 低 | `scripts/update_nfr_baseline.sh` 経由のみで更新 |
| `.github/workflows/` | CI/CD定義 | 低 | 変更は慎重に、`deny.toml`/nextestプロファイルと連動 |

## 主要ファイルの役割

### core-runtime/src/
- `lib.rs` — クレートルート、公開API再エクスポート
- `browser.rs` — `BrowserClient`(Chromeプロセス管理)、`PageSession`(タブ単位の操作: act/verify/capture/policy/audit/recovery)
- `chrome_detection.rs` — Chromeバイナリ検出
- `dom_signature.rs` — Self-Healing Context Recovery(`stable_key`照合失敗時のフォールバック)
- `policy.rs` — `PolicyEngine`, `PolicyRule`, `PolicyDecision`, Guardian Angel (`OutcomeProjection`)
- `privacy.rs` — PII検出/redaction正規表現
- `prompt_injection.rs` — プロンプトインジェクション検出/redaction (`PromptInjectionSanitizer`)
- `audit.rs` / `audit_sink.rs` / `audit_replay.rs` — 構造化監査ログ、永続化シンク、リプレイ
- `session_vault.rs` — セッション認証情報の暗号化保存
- `plugin_hooks.rs` — プラグイン用フックトレイト境界
- `speculative/{mod,model,codec}.rs` — 投機的状態生成パイプライン
- `sre/{state,normalization,pipeline,profile,stable_key}.rs` — Semantic Rendering Engine本体

### mcp-server/src/
- `main.rs` — バイナリエントリポイント
- `lib.rs` — ツール定義/ディスパッチ、使用量メータリング、投機的状態の組み込み(巨大ファイル)
- `config.rs` — `config.toml` 読み込み + 環境変数オーバーライド
- `doctor.rs` — `--doctor` の検証ロジック
- `init.rs` — `--init <client>` のスニペット生成
- `cli.rs` — CLI引数解析

### skills-engine/src/
- `lib.rs` — `SkillDefinition`, `SkillStep`(Locate/Verify/Act/Wait/Extract/Handoff), `SkillEngine`/`SkillRuntime`

### plugin-host/src/
- `lib.rs` — `PluginManifest`, `PluginRuntime`(wasmtime実行)
- `schema_registry.rs` — 事前コンパイル済み抽出ルールのレジストリ

### hitl-bridge/src/
- `main.rs` — Slack HITLブリッジCLIエントリ
- `server.rs` — `POST /slack/interactions` 受信、HMAC検証
- `bridge.rs` — ゲートウェイポーリング→通知→解決のオーケストレーション
- `lock.rs` — 二重解決防止の排他ロック
- `gateway.rs` — ブラウザセッションへの抽象化
- `notifier.rs` — 通知送信トレイト
- `audit.rs` — HITL承認の追記専用監査トレイル

## 機能別に見るべき場所

### MCPツールの挙動を変更するとき
- `mcp-server/src/lib.rs`(ツール定義・ディスパッチ・メータリング)
- `mcp-server/tests/mcp_protocol_compliance.rs`, `mcp_client_contract.rs`, `mcp_schema_*.rs`
- `examples/mcp_examples/*.json`(リクエスト/レスポンスサンプル、必要なら更新)
- `README.md`「Available MCP Tools」表

### Semantic State / DOM正規化を変更するとき
- `core-runtime/src/sre/normalization.rs`, `sre/pipeline.rs`, `sre/state.rs`
- `core-runtime/tests/sre_determinism.rs`, `sre_snapshot_regression.rs`, `sre_fast_full_state.rs`
- `core-runtime/tests/fixtures/golden/*.json`, `fixtures/sre/minimal_regression_snapshot.json`

### `stable_key` / 要素識別を変更するとき
- `core-runtime/src/sre/stable_key.rs`
- `core-runtime/tests/stable_key_*.rs`(generation/compliance/compatibility/quadrant_alias_index)
- `core-runtime/tests/spa_stable_key_stress.rs`

### ポリシー/承認ロジックを変更するとき
- `core-runtime/src/policy.rs`
- `core-runtime/tests/policy_engine.rs`, `policy_enforcement.rs`, `policy_schema_lint.rs`
- `examples/policy_cookbook.rs`, `examples/sample_policy.json`

### HITL(人間承認)を変更するとき
- `hitl-bridge/src/*.rs`
- `mcp-server/src/lib.rs` の `ask_human`/HITLイベント計測部分
- `hitl-bridge/tests/bridge_flow.rs`
- `mcp-server/tests/mcp_hitl_flow.rs`
- `docs/hitl-slack-bridge.md`

### 投機的状態生成(Speculative State Generation)を変更するとき
- `core-runtime/src/speculative/{mod,model,codec}.rs`
- `core-runtime/tests/speculative_pregeneration.rs`
- `mcp-server/tests/speculative_get_state_ttft.rs`

### 監査ログ/プライバシーを変更するとき
- `core-runtime/src/audit.rs`, `audit_sink.rs`, `audit_replay.rs`, `privacy.rs`
- `core-runtime/tests/audit_logging.rs`, `audit_persistence.rs`, `audit_schema.rs`, `pii_redaction.rs`, `pii_injection_composition.rs`
- `scripts/audit_replay.py`

### プロンプトインジェクション対策を変更するとき
- `core-runtime/src/prompt_injection.rs`
- `core-runtime/tests/prompt_injection_pipeline.rs`
- `README.md`「Security: Prompt Injection Sanitization」

### Wasmプラグインを変更するとき
- `plugin-host/src/lib.rs`, `schema_registry.rs`
- `core-runtime/src/plugin_hooks.rs`
- `plugin-host/tests/plugin_runtime.rs`, `plugin_signature_verification.rs`, `plugin_sbom_validation.rs`

### スキル(宣言的ワークフロー)を変更するとき
- `skills-engine/src/lib.rs`
- `skills-engine/tests/skill_conformance.rs`, `skill_schema_compatibility.rs`
- `skills-engine/tests/fixtures/skill/skill_definition.schema.json`
- `examples/sample_skill.json`

### Chromeクラッシュ復旧/接続管理を変更するとき
- `core-runtime/src/browser.rs`(`relaunch`, `is_browser_disconnected`)
- `core-runtime/tests/browser_recovery.rs`, `cdp_connectivity.rs`
- `mcp-server/tests/mcp_browser_recovery.rs`

### 課金/使用量メータリングを変更するとき
- `mcp-server/src/lib.rs`(`UsageMeters`, `UsageReport`, `PlanTier`)
- `mcp-server/tests/mcp_billing_plan_gating.rs`, `mcp_usage_metering_gaps.rs`, `mcp_pricing_snapshot.rs`

### NFR/パフォーマンスベンチマークを変更するとき
- `bench/src/{harness,metrics,report}.rs`
- `core-runtime/tests/nfr_*.rs`
- `nfr-baseline/*.json`(`scripts/update_nfr_baseline.sh` 経由のみ更新)
- `scripts/nfr_trend.py`, `scripts/nfr_dashboard.py`

## 変更してはいけない、または慎重に扱うべきファイル

- `Cargo.lock` — 手動編集禁止。`cargo build`/`cargo update` で再生成。
- `target/` 以下すべて — ビルド出力(`nfr-dashboard*.md`, `som-artifacts/*.png` 含む)。
- `nfr-baseline/*.json` — `scripts/update_nfr_baseline.sh` 経由でのみ更新。
- `core-runtime/tests/fixtures/som/som_visual_baseline.png` などのバイナリ/スナップショットフィクスチャ — 意図した視覚差分更新以外では変更しない。
- `.github/workflows/*.yml` — `deny.toml`、`.config/nextest.toml` と連動するため変更は影響範囲を確認してから。
- `deny.toml` — RUSTSEC ID除外には必ず根拠コメントが付与されている。削除/追加は理由を明記する。

## 自動生成ファイル・設定ファイルの扱い

| ファイル | 種別 | 扱い |
|---|---|---|
| `Cargo.lock` | 自動生成 | 編集禁止、コマンドで再生成 |
| `target/**` | ビルド出力 | 無視、コミットしない |
| `core-runtime/target/nfr-dashboard*.md` | ベンチ出力 | 無視 |
| `rust-toolchain.toml` | 設定(`channel = "stable"`) | 変更時はCI全体に影響、慎重に |
| `deny.toml` | 設定(cargo-deny advisories) | 例外追加時は根拠を明記 |
| `.config/nextest.toml` | 設定(`default`/`ci`プロファイル) | プロファイル非継承に注意。`ci` は `default` を継承しない |
| `nfr-baseline/*.json` | ベースラインデータ | スクリプト経由のみ更新 |
