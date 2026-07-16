# Neural-Browser Runtime 実装計画（進捗管理版）

- 対象仕様: [SPEC.md](./SPEC.md) v2.1（2026-02-10）
- 最終更新日: 2026-07-06
- プラン状態: In Progress

## 1. 進捗管理ルール

- `Status` は `NOT_STARTED` / `IN_PROGRESS` / `BLOCKED` / `DONE` を使用する。
- このファイルは実装計画と完了済みPRの履歴であり、現在のコードベースの正本は
  `Cargo.toml`、各 crate の実装、`mcp-server/src/lib.rs` の MCP tool contract、
  および `README.md` / `docs/ARCHITECTURE.md` の利用者向け説明で確認する。
- 各PRは `実装` / `テスト` / `CI` の3カテゴリでチェックボックス管理する。
- PRを `DONE` に変更できる条件:
  - 実装タスク完了
  - テストタスク完了
  - CIタスク完了
  - 受け入れ条件（Exit Criteria）を満たす
- 進捗率は `完了チェック数 / 全チェック数` で算出する。
- フェーズダッシュボードの `Progress` は `DONE PR数 / フェーズ内PR総数` で算出する。
- フェーズダッシュボードの `Status` は `DONE` / `IN_PROGRESS` / `NOT_STARTED` を使用する。

### MVP完了条件

MVPは「外部クライアントから安全に利用可能な Neural-Browser Runtime の最小提供ライン」と定義し、次をすべて満たした時点で完了とする。

- [x] 必須PRがすべて `DONE` である（`PR-00`〜`PR-11`, `PR-14`）。
- [x] 必須CIチェック（`lint`, `test`, `smoke-e2e`, `cdp-smoke`, `policy-regression`, `policy-schema-lint`, `sre-regression`, `sre-semantic-delta`, `som-regression`, `mcp-protocol-compliance`, `mcp-api-schema-diff`, `mcp-schema-compatibility`）が安定してグリーンである。
- [x] `SEC-01` / `SEC-02` / `AUD-01` の Exit Criteria（`PR-09`〜`PR-11`）がすべて満たされている。
- [x] `PR-14` の Exit Criteria（MCPツール群の一貫利用）が満たされ、外部契約テストがCI必須化されている。
- [x] 利用手順・既知制約・障害時手順が `docs/` に明示され、運用可能な状態になっている。

## 2. フェーズダッシュボード

| Phase | Scope | PRs | Progress | Status |
| :--- | :--- | :--- | :--- | :--- |
| 0 | テスト・CI基盤 | PR-00 | 1/1 | DONE |
| 1 | Core Runtime + SRE基盤 | PR-01〜03 | 3/3 | DONE |
| 2 | Interaction & Reliability | PR-04〜06 | 3/3 | DONE |
| 3 | Performance & NFR | PR-07〜08, PR-15 | 3/3 | DONE |
| 4 | Security & Audit | PR-09〜11 | 3/3 | DONE |
| 5 | Extensions & API | PR-12〜14 | 3/3 | DONE |
| 6 | Monetization Meters | PR-16 | 1/1 | DONE |
| 7 | Marketplace | PR-17 | 1/1 | DONE |
| 8 | Robustness & Verification | PR-18 | 1/1 | DONE |
| 9 | Comprehensive Evaluation Bench | PR-19 | 1/1 | DONE |
| 10 | Cathedral Edition (Commercialization) | PR-20〜29 | 10/10 | DONE |

## 3. PRバックログ（進捗チェック付き）

### PR-20: Speculative State Generation Pipeline
- Status: `DONE`
- Spec Ref: Section 3.5, ISSUE-10
- Dependencies: PR-08, PR-14
- 実装タスク
  - [x] `core-runtime/src/speculative/mod.rs` を新設し予測エンジンを実装。
  - [x] `flatbuffers` によるモデルのシリアライズ/デシリアライズを実装。
  - [x] 予測失敗時の `StateDelta::Mismatch` とリプレイ・デバッグ用のスナップショット保存を実装。
- テストタスク
  - [x] 意図的な予測ミス（Drift Injection）によるバックトラッキングの検証。
- Exit Criteria
  - [x] 予測的中時に TTFT < 10ms を達成。

> **Follow-up (ISSUE-147, done)**: `SpeculativeEngine` を `mcp-server::CoreRuntimeBackend`
> の `get_state`/`act` に接続。`act` の実行アクションを `ActionSignature` として記録し、
> `get_state` 呼び出し時に `resolve_speculative_state` で予測検証・キャッシュ済み
> スナップショットの提供 (`metadata.speculative: true`) または通常キャプチャへの
> フォールバックを判定する。`get_usage_report` に `state_generations.speculative` /
> `speculative_misses` を追加し、ヒット/ミス計測を可視化。Delta 配信経路・
> `mismatch_log` の外部提示・スキル実行中の `pending_action` 追跡は範囲外
> （別途検討）。E2E TTFT 検証: `mcp-server/tests/speculative_get_state_ttft.rs`。

### PR-21: Self-Healing Context Recovery Layer
- Status: `DONE`
- Spec Ref: Section 3.6, ISSUE-11
- Dependencies: PR-03, PR-04
- 実装タスク
  - [x] `DOMSignatureCache` による成功操作パターンの永続化。
  - [x] `stable_key` 喪失時のファジーマッチング修復ロジックの実装。
  - [x] 修復不能時の自動 `ask_human` フォールバック。
- テストタスク
  - [x] 大幅な UI 変更 fixture に対する自動修復成功率の検証。
- Exit Criteria
  - [x] 修復成功時に `verify` 要求なしでアクションが継続される。

### PR-22: "Deep Lens" Zero-Code Extraction DSL
- Status: `DONE`
- Spec Ref: Section 4.3, ISSUE-12
- Dependencies: PR-12, PR-14
- 実装タスク
  - [x] YAML/JSON ベースの抽出 DSL パーサーと `SchemaRegistry` の実装。
  - [x] Wasm Plugin Host 経由での型安全な抽出実行。
- テストタスク
  - [x] `Golden Dataset` を用いた抽出精度（Accuracy 100%）の検証。
- Exit Criteria
  - [x] スクリーンスクレイピング・コードなしでの構造化データ取得が可能。

### PR-23: "Guardian Angel" & Outcome Projection
- Status: `DONE`
- Spec Ref: Section 3.7, ISSUE-13
- Dependencies: PR-09, PR-14
- 実装タスク
  - [x] `PolicyEngine` への副作用予測（Outcome Projection）シミュレータ統合。
  - [x] 予測データに基づくプロアクティブな実行ブロックと HITL 通知。
- テストタスク
  - [x] 高額決済等の閾値超過シナリオでの自動ブロックと通知内容の検証。
- Exit Criteria
  - [x] アクション実行前に構造化された副作用データが提示される。

### PR-24: Persistent Audit Hardening (Rolling File & SIEM)
- Status: `DONE`
- Spec Ref: Section 3.4, ISSUE-09, ISSUE-14
- Dependencies: PR-10
- 実装タスク
  - [x] `RollingFileSink` と `WebhookSink` (SIEM) の実装。
  - [x] 非同期ログ書き込みの信頼性向上（Retry / Backpressure）。
- テストタスク
  - [x] 高負荷バースト時のログ欠損ゼロ検証。
- Exit Criteria
  - [x] 長期保存可能な監査トレースが外部システムと連携される。

### PR-25: Slack/Teams HITL Reference Implementation
- Status: `DONE`
- Spec Ref: Section 5.2, ISSUE-15
- Dependencies: PR-14, PR-23
- 実装タスク
  - [x] Slack App 連携リファレンス実装（画像 + 未来投影データ付き）。
  - [x] セッションレベルの二重承認防止ロック。
- テストタスク
  - [x] 複数人による同時承認リクエストの競合回避検証。
- Exit Criteria
  - [x] チャットツール経由で安全に HITL 判断を完走できる。

### PR-26: Shared Wasm Engine & Performance Tuning
- Status: `DONE`
- Spec Ref: Section 4.1, ISSUE-16
- Dependencies: PR-12
- 実装タスク
  - [x] `wasmtime` Engine のグローバル共有とコンパイル済みモジュールのキャッシュ。
  - [x] Epoch-based Interruption による暴走プラグインの強制遮断。
- テストタスク
  - [x] 大量プラグインロード時のメモリ消費と起動レイテンシの計測。
- Exit Criteria
  - [x] プラグイン起動時間が 1ms 以下に短縮。

> **Follow-up (ISSUE-209)**: `RollingFileSink`、Wasm module cache、
> `SpeculativeEngine` の mutex poison をサブシステム別に安全回復する。
> 監査書き込みは当該イベントを明示エラーにして新規ファイルへ切り替え、
> disposable cache は再構築し、予測状態は cold reset して通常 capture へフォールバックする。

### PR-27: Unified PII Redactor Utility
- Status: `DONE`
- Spec Ref: Section 3.4, ISSUE-08, ISSUE-17
- Dependencies: PR-02, PR-10
- 実装タスク
  - [x] `core-runtime/src/privacy.rs` へのマスクロジック集約と強制フック実装。
- テストタスク
  - [x] あらゆる経路（SRE/Audit/Deep Lens）でのマスク漏れゼロ検証。
- Exit Criteria
  - [x] 機密データが AI モデルやログに未加工で流れないことが保証される。

### PR-28: Side-by-side ROI Comparison Tool
- Status: `DONE`
- Spec Ref: Section 8.1, ISSUE-18
- Dependencies: PR-14, PR-15
- 実装タスク
  - [x] Playwright 比較ベンチマーク・ハーネスの実装。
  - [x] トークン・コスト削減効果の自動レポート生成。
- テストタスク
  - [x] 代表的な EC/業務サイトでの削減率実測（browser `#[ignore]` テスト追加済）。
- Exit Criteria
  - [x] 導入メリットを定量的に示す Markdown レポートが出力可能。

### PR-29: config.toml Runtime Configuration Loading
- Status: `DONE`
- Spec Ref: ISSUE-146
- Dependencies: PR-09, PR-24
- 実装タスク
  - [x] `mcp-server/src/config.rs` で `$XDG_CONFIG_HOME/dragon-head/config.toml`（`$HOME/.config` フォールバック）の読み込みと解決を実装。
  - [x] `chrome_path` / `prompt_injection.mode` / `policy.file` / `audit.*` を環境変数優先で解決し、`main.rs` の起動シーケンスに統合。
  - [x] `CoreRuntimeBackend::set_injection_mode` で起動時に `PromptInjectionSanitizer` を再構成。
  - [x] `--doctor` が `config.toml` の存在確認に加え、パース失敗・不正な `prompt_injection.mode` を fatal として検出。
  - [x] `--init` 出力と README に `config.toml` のスキーマと優先順位表を追記。
- テストタスク
  - [x] `config.rs` 単体テスト16件（パス解決・読み込み・優先順位・不正値）。
  - [x] `doctor.rs` の config チェック3件（malformed/invalid mode/有効ファイルの解決サマリ）。
  - [x] バイナリE2E `--doctor` テスト2件（`XDG_CONFIG_HOME` 経由で resolve_config の結果を検証）。
- Exit Criteria
  - [x] `config.toml` で `chrome_path` / `prompt_injection.mode` / `policy.file` / `audit.*` がユーザー設定可能になり、`--doctor` がその内容を検証する。

### PR-30: Chrome Crash/Disconnect Recovery in Long-Running MCP Sessions
- Status: `DONE`
- Spec Ref: ISSUE-149
- Dependencies: PR-21
- 実装タスク
  - [x] `SessionError::{BrowserRestarted, BrowserRestartFailed}`（`core-runtime/src/error.rs`）と `AuditEvent::BrowserRestart`（`audit.rs`）を追加。
  - [x] `is_browser_disconnected`（`headless_chrome::browser::ConnectionClosed` の型判定 + IOマーカーのフォールバック）と `BrowserClient::{process_id, relaunch}` を実装。
  - [x] `mcp-server`: `McpServer::call_tool` が disconnect を検出した呼び出しを `CoreRuntimeBackend::handle_browser_disconnect` にディスパッチし、`SessionError::BrowserRestarted`/`BrowserRestartFailed` を `-32000` で返却。
  - [x] 60秒に3回までのレート制限（`RESTART_RATE_LIMIT_MAX`/`RESTART_RATE_LIMIT_WINDOW`）で再起動ストームを防止。
  - [x] `get_usage_report` に `browser_restarts` フィールドを追加。
  - [x] `main.rs` を `CoreRuntimeBackend::new_with_client` / `set_policy_rules` に更新し、再起動後のページにポリシーを再適用。
- テストタスク
  - [x] `error.rs` / `browser.rs` の単体テスト6件（メッセージ・disconnect判定・relaunch構成）。
  - [x] `mcp-server/src/lib.rs` の単体テスト5件（call_tool の disconnect 介入・レート制限対象外・usage report）。
  - [x] 統合テスト: Chrome プロセスを kill して `process_id()` の変化・`relaunch` 後のページ動作・`call_tool` のリカバリ・レート制限到達を検証（`core-runtime/tests/browser_recovery.rs`, `mcp-server/tests/mcp_browser_recovery.rs`、`should_skip_browser_tests` でガード）。
- Exit Criteria
  - [x] Dead/disconnected な Chrome プロセスをページレベルエラーと区別して検出できる（型付きエラー）。
  - [x] 検出時に自動再起動 + 新規ページを試行し、進行中の呼び出しに対してナビゲーション状態・Cookie・承認状態がリセットされたことを伝える構造化 JSON-RPC エラーを返す。
  - [x] `get_usage_report` に再起動回数を表すフィールドを追加する。
  - [x] 統合テスト: 実行中セッションで Chrome プロセスを kill し、直後の `get_state` が回復することを確認する（既存の browser-test skip helper でガード）。

### PR-00: Test Strategy & CI Foundation
- Status: `DONE` (Local)
- Spec Ref: Section 6（NFR 全体の測定可能性）
- Dependencies: None
- 実装タスク
  - [x] Rust workspace共通の `fmt` / `clippy` / `test` 実行ターゲットを定義する。
  - [x] GitHub Actions `ci.yml` を作成し、`lint`・`unit`・`integration` を必須チェック化する。
  - [x] E2E向けに headless Chromium を使う `e2e.yml`（PR時はsmoke、nightlyはfull）を追加する。
  - [x] 失敗時にログ・スクリーンショット・トレースをartifactとして保存する。
- テストタスク
  - [x] テスト分類（unit/integration/e2e/perf/security）と配置ルールを `docs/testing.md` に定義する。
  - [x] サンプルテストを各レイヤーに1件ずつ追加し、CIで実行確認する。
- CIタスク
  - [x] ブランチ保護ルールに Required Checks を設定する（`lint`, `unit`, `integration`, `smoke-e2e`）。
  - [x] カバレッジ収集を有効化し、しきい値（例: 70%）未満で失敗させる。
- Exit Criteria
  - [x] 新規PRで最低1つのテストがなければCIが失敗する状態になっている。


### PR-01: Project Initialization & CDP Client Wrapper
- Status: `DONE` (Local)
- Spec Ref: Section 2.1（Layer 1）
- Dependencies: PR-00
- 実装タスク
  - [x] Rust workspace（`core-runtime`, `plugin-host`, `skills-engine`）を初期化する。
  - [x] CDP接続ラッパー（接続/再接続/切断）を実装する。
  - [x] `PageSession`（CDPセッションとページコンテキスト保持）を実装する。
- テストタスク
  - [x] `example.com` への遷移とHTML取得のintegration testを追加する。
  - [x] CDP切断時の再接続挙動を検証するテストを追加する。
- CIタスク
  - [x] CIに `cdp-smoke` ジョブを追加し、Linux runner上で常時実行する。
  - [x] flaky検知のため `cdp-smoke` を2回連続実行する設定を追加する。
- Exit Criteria
  - [x] ヘッドレスブラウザ起動からHTML取得までを安定して実行できる。

### PR-02: SRE-01 Deterministic State Generation
- Status: `DONE` (Local)
- Spec Ref: SRE-01, Section 5.1
- Dependencies: PR-01
- 実装タスク
  - [x] `minimal` / `visual` / `interactive` のLoad Profileを実装する。
  - [x] 正規化処理（動的クラス除去、広告除外）を実装する。
  - [x] `Fast State`（`interactive_elements`, `messages`）を優先生成する。 (Note: Included in Full State for now)
  - [x] `Full State`（`forms`, `regions`）をバックグラウンド生成する。
  - [x] `metadata` に `page_instance_id`, `state_hash`, `timestamp`, `load_profile` を含める。
- テストタスク
  - [x] fixture HTMLに対するdeterministic出力テストを追加する。
  - [x] Profile別のリソース制御（ブロック/許可）テストを追加する。
  - [x] Fast/Full Stateの内容差分と生成順序のテストを追加する。
- CIタスク
  - [x] SRE fixture回帰テストをCI必須ジョブ化する。
  - [x] 仕様変更時のスナップショット更新をPR内で強制するチェックを追加する。 (`sre_snapshot_regression` をCIジョブ化)
- Exit Criteria
  - [x] 同一入力に対し `state_hash` が再現性を持つ。

### PR-03: ACT-01 Stable Key Generation
- Status: `DONE` (Local)
- Spec Ref: ACT-01, Section 5.1
- Dependencies: PR-02
- 実装タスク
  - [x] `sha256(role + normalized_label + dom_signature + quadrant)` を実装する。 (Quadrant omitted for now as per plan)
  - [x] `stable_key`（不変ID）と `alias`（人間可読名）を分離する。
  - [x] 衝突時のインデックス付与と `ambiguous: true` を実装する。 (Index appended, ambiguous flag in struct)
  - [x] `stable_key -> Node` インデックスをメモリ常駐化する。 (Implicit in traversal, full index requires separate struct but core logic is done)
  - [x] `quadrant` を stable key計算入力に正式導入し、同一 `dom_signature` 要素の識別精度を上げる。
  - [x] `alias` を生成して `interactive_elements` の公開出力へ反映する。
  - [x] `stable_key -> Node` 常駐インデックス（`HashMap`）を `PageSession` 単位で保持し、探索を O(1) 化する。
- テストタスク
  - [x] DOM再レンダリング時のキー安定性テストを追加する。
  - [x] 衝突ケースで `ambiguous` が正しく立つことを検証する。 (Collision handling verified)
  - [x] quadrant差分で key が変わり、同一再レンダリングでは不変であることを検証する。
  - [x] `alias` 出力と `stable_key` インデックスの整合性テストを追加する。
- CIタスク
  - [x] stable key回帰テストをCI必須化する。 (Included in workspace tests)
  - [x] ハッシュ計算ロジックの変更時に互換性テストを必須化する。 (`stable_key_compatibility` をCIジョブ化)
- Exit Criteria
  - [x] fallback探索に必要な `stable_key` インデックスが常に利用可能。

### PR-04: ACT-04 Robust Action Execution
- Status: `DONE` (Local)
- Spec Ref: ACT-04
- Dependencies: PR-03
- 実装タスク
  - [x] `act`（`click`, `type`）を `target_id` 優先で実装する。
  - [x] `target_id` 失敗時に `target_stable_key` fallbackを実装する。
  - [x] fallback成功時にWarningログを出力する。
  - [x] 両方失敗時に `verify` 要求（`ActionError::VerifyRequired`）を返すフローを実装する。
- テストタスク
  - [x] `target_id` 無効化時に `stable_key` で復旧するintegration testを追加する。
  - [x] 二重失敗時に `verify required` を返すことを確認する。
- CIタスク
  - [x] action回帰テストをPRごとに実行するジョブを追加する。 (Included in workspace tests)
  - [x] 失敗ケースのログ構造（warning/error）検証を自動化する。 (`action_execution` に構造化ログ検証を追加)
- Exit Criteria
  - [x] ACT-04 Recovery Flow（1→2→3）をテストで再現できる。

### PR-05: ACT-03 Semantic Wait
- Status: `DONE` (Local)
- Spec Ref: ACT-03
- Dependencies: PR-04
- 実装タスク
  - [x] `wait_for_semantic(target, state)` を実装する。
  - [x] `wait_for_intent(intent)` を実装する。
  - [x] SRE Queueイベントを購読して待機解除を行う。
  - [x] `SRE Queue` からの pushイベント購読を導入し、定周期ポーリング依存を除去する。
- テストタスク
  - [x] 遅延ロードページで `enabled` 待機が機能するE2Eを追加する。
  - [x] intent成立/不成立のタイムアウト挙動テストを追加する。
  - [x] イベント駆動経路で待機解除され、固定間隔ポーリングに依存しないことを検証する。
- CIタスク
  - [x] semantic wait E2Eをsmoke対象に含める。
  - [x] タイムアウトが閾値を超える場合に失敗する性能アサーションを追加する。
- Exit Criteria
  - [x] 固定sleepに依存しない待機APIが利用可能。

### PR-06: ACT-02 Event-Driven SoM
- Status: `DONE` (Local)
- Spec Ref: ACT-02
- Dependencies: PR-02, PR-04
- 実装タスク
  - [x] 常時SoM生成を廃止し、イベント駆動パイプラインに切り替える。
  - [x] トリガー条件（`get_visual`, `act` ambiguous, `verify` failure）を実装する。
  - [x] `marks`（ID/BBox/stable_key対応）付き出力を実装する。
- テストタスク
  - [x] 非トリガー時にSoMが生成されないことを検証する。
  - [x] トリガー3種ごとに `marks` 整合性を検証する。
- CIタスク
  - [x] SoM生成テストでスクリーンショット差分をartifact保存する。
  - [x] 画像差分しきい値超過時に失敗するチェックを追加する。
- Exit Criteria
  - [x] SoM生成回数がイベント発火時に限定される。

### PR-07: SRE-02 Semantic Delta (RFC 6902)
- Status: `DONE` (Local)
- Spec Ref: SRE-02
- Dependencies: PR-02
- 実装タスク
  - [x] `state_hash` 差分に基づく JSON Patch（RFC 6902）生成を実装する。
  - [x] MutationObserver連携で変更サブツリーのみ再解析する。
  - [x] Full StateとDeltaの切り替えポリシーを実装する。
- テストタスク
  - [x] 軽微DOM変更でpatchサイズが縮小することを検証する。
  - [x] patch適用後の再構築Stateが原本一致することを検証する。
- CIタスク
  - [x] RFC 6902準拠テストスイートをCI必須化する。
  - [x] patchサイズ回帰（肥大化）検出チェックを追加する。
- Exit Criteria
  - [x] Delta送信でフル再送を回避できる。

### PR-08: Async Pipeline Architecture
- Status: `DONE` (Local)
- Spec Ref: Section 2.2, Section 6（TTFT）
- Dependencies: PR-07
- 実装タスク
  - [x] `Render Queue`, `SRE Queue`, `Audit/Policy Queue` を非同期分離する。
  - [x] `Fast State` 優先スケジューリングを実装する。
  - [x] Queue間のバックプレッシャー制御を実装する。
- テストタスク
  - [x] 高負荷下でもデッドロックしないことを検証する負荷テストを追加する。
  - [x] TTFT計測テスト（Fast State条件）を追加する。
- CIタスク
  - [x] PRでは短時間ベンチ、nightlyでは長時間ベンチを実行する。
  - [x] TTFTの回帰を検知して警告/失敗にするしきい値を設定する。
- Exit Criteria
  - [x] Queue分離後も機能退行がなく、TTFT目標達成の見込みが示せる。

### PR-09: SEC-01 Context-Aware Policy Engine
- Status: `DONE` (Local)
- Spec Ref: SEC-01
- Dependencies: PR-04
- 実装タスク
  - [x] Policyルール定義（domain/path/role/text/context）を実装する。
  - [x] `allow` / `block` / `require_human_approval` の判定を実装する。
  - [x] approval scope（`action_only`, `until_navigation`, `timeboxed(ms)`）を実装する。
- テストタスク
  - [x] ルール条件ごとの判定テストを追加する。
  - [x] scopeの有効期限・navigation跨ぎの挙動テストを追加する。
- CIタスク
  - [x] policy regression suiteを必須ジョブ化する。
  - [x] ルールファイルの静的検証（schema lint）を追加する。
- Exit Criteria
  - [x] 重要アクションに対する事前審査が必ず実行される。

### PR-10: AUD-01 Structured Audit Log & PII Redaction
- Status: `DONE` (Local)
- Spec Ref: AUD-01
- Dependencies: PR-08
- 実装タスク
  - [x] Event Model（`STATE_SNAPSHOT`, `STATE_PATCH`, `TOOL_CALL`, `POLICY_DECISION`, `HITL_EVENT`, `VISUAL_CAPTURE`）を実装する。
  - [x] 非同期ログ書き込みを実装する。
  - [x] PIIマスク（`password/email` フィールド、クレジットカード形式）を実装する。
- テストタスク
  - [x] 各イベントタイプのスキーマ検証テストを追加する。
  - [x] 機密入力が必ずマスクされるテストを追加する。
- CIタスク
  - [x] 監査ログスキーマ互換性チェックを必須化する。
  - [x] PII漏洩検知テストを必須化する。
- Exit Criteria
  - [x] 再現可能な監査トレースを出力できる。

### PR-11: SEC-02 Session Vault & Key Management
- Status: `DONE`
- Spec Ref: SEC-02
- Dependencies: PR-01
- 実装タスク
  - [x] Cookie/TokenのAES-256暗号化保存を実装する。
  - [x] `SessionVault` 抽象とローカル実装を追加する。
  - [x] BYOK対応インターフェース（KMSアダプタ）を設計する。
  - [x] 鍵ローテーションAPIを実装する。
- テストタスク
  - [x] 保存データが平文でないことを検証する。
  - [x] 鍵ローテーション後の復号互換テストを追加する。
- CIタスク
  - [x] mock KMSを用いたvault integration testを追加する。
  - [x] 暗号関連依存の脆弱性スキャンをCIに追加する。
- Exit Criteria
  - [x] Local鍵とBYOKの双方でセッション再利用が可能。

### PR-12: PLUG-01/02 Plugin Framework (Wasm)
- Status: `DONE` (Local)
- Spec Ref: PLUG-01, PLUG-02
- Dependencies: PR-02, PR-09
- 実装タスク
  - [x] Wasmランタイム（`wasmtime` 等）を導入する。
  - [x] Extension Point（`on_state`, `before_act`, connector）を定義する。
  - [x] capability manifest（`read_state`, `network_out`, `vault_access`）を実装する。
  - [x] 署名済みpluginのみロード可能にする。
  - [x] SBOM提出/検証フローを実装する。
- テストタスク
  - [x] 非署名pluginが拒否されることを検証する。
  - [x] capability逸脱アクセスが遮断されることを検証する。
- CIタスク
  - [x] plugin署名検証ジョブを必須化する。
  - [x] SBOM検証ジョブを必須化する。
- Exit Criteria
  - [x] サンドボックス制約下で安全にplugin実行できる。

### PR-13: SKILL-01 Skills Engine
- Status: `DONE` (Local)
- Spec Ref: SKILL-01
- Dependencies: PR-04, PR-09
- 実装タスク
  - [x] Skill JSON schema（`locate`, `verify`, `act`, `wait`, `extract`, `handoff`）を定義する。
  - [x] 実行順序 `verify -> policy_check -> act -> post_check` を強制する。
  - [x] リトライ・分岐・handoff処理を実装する。
- テストタスク
  - [x] 正常系ワークフロー（例: 検索〜抽出）を追加する。
  - [x] `verify` 失敗時に `act` が抑止されるテストを追加する。
- CIタスク
  - [x] skill conformance testを必須ジョブ化する。
  - [x] schema変更時の後方互換性チェックを追加する。
- Exit Criteria
  - [x] 宣言的Skillで再現性あるタスク実行が可能。

### PR-14: MCP Tool Interface
- Status: `DONE` (Local)
- Spec Ref: Section 5.2
- Dependencies: PR-04, PR-05, PR-06, PR-09, PR-13
- 実装タスク
  - [x] MCPサーバを実装し内部APIを公開する。
  - [x] `get_state`, `act`, `verify`, `get_visual`, `ask_human`, `run_skill` を公開する。
  - [x] 各tool引数・戻り値を仕様準拠に揃える（`force_refresh` など）。
  - [x] `get_state(format=json)` の戻り値を Section 5.1 スキーマ（`metadata.url`, `interactive_elements[].id/stable_key/alias/role/name/attributes/bbox/policy_flags`）に厳密準拠させる。
  - [x] 内部表現（`SemanticState`）と外部公開DTOを分離し、スキーマ互換性を管理する。
- テストタスク
  - [x] MCP clientとの契約テストを追加する。
  - [x] `ask_human` を含むHITLフローのE2Eテストを追加する。
  - [x] Section 5.1 JSONサンプルに対するスキーマ準拠（golden/contract）テストを追加する。
- CIタスク
  - [x] MCP protocol complianceテストを必須化する。
  - [x] API schema差分の自動検知を追加する。
  - [x] JSON Schema互換性チェック（後方互換）を必須化する。
- Exit Criteria
  - [x] 仕様ツール群を外部クライアントから一貫利用できる。

### PR-15: NFR Benchmark & Capacity Validation
- Status: `DONE` (Local)
- Spec Ref: Section 6
- Dependencies: PR-08, PR-10, PR-14
- 実装タスク
  - [x] TTFT `<50ms`（Fast State, Minimal Profile条件）計測基盤を実装する。
  - [x] State Update Latency `<100ms`（変更ノード < 50条件）を計測する。
  - [x] 帯域95%削減（Standard比）を計測する。
  - [x] 容量指標（Minimal 75 sessions/instance, Visual 20 sessions/instance）を測定する。
- テストタスク
  - [x] 再現可能なベンチシナリオと負荷プロファイルを固定化する。
  - [x] 結果の統計妥当性（複数試行、p95/p99）を検証する。
- CIタスク
  - [x] PRでは短尺性能テスト、nightlyではフル性能テストを実行する。
  - [x] NFR回帰ダッシュボードを生成し、しきい値超過時に失敗させる。
- Exit Criteria
  - [x] 主要NFR指標が計測可能かつ継続監視可能。

### PR-16: Billing Meters & Plan Gating
- Status: `DONE` (Local)
- Spec Ref: Section 7.1, 7.2
- Dependencies: PR-10, PR-14
- 実装タスク
  - [x] Meter（`State Generations`, `Visual Captures`, `Actions Executed`, `HITL Events`, `Audit Retention`）を実装する。
  - [x] プラン別feature gate（Developer/Pro/Enterprise）を実装する。
  - [x] 利用量レポートAPIを実装する。
- テストタスク
  - [x] メーター計測精度テストを追加する。
  - [x] プラン境界のアクセス制御テストを追加する。
- CIタスク
  - [x] メータリング回帰テストを必須化する。
  - [x] 料金計算スナップショット差分チェックを追加する。
- Exit Criteria
  - [x] 仕様の課金メーターが再現性を持って収集できる。

### PR-17: Marketplace Integration
- Status: `DONE` (Local)
- Spec Ref: Section 7.3
- Dependencies: PR-12, PR-16
- 実装タスク
  - [x] Domain Pack（plugin + skill bundle）のパッケージ仕様を定義する。
  - [x] Marketplace向け公開メタデータ（署名、互換バージョン、依存情報）を実装する。
  - [x] Revenue Share集計に必要な利用イベント連携を実装する。
- テストタスク
  - [x] Bundleの署名検証・互換性検証テストを追加する。
  - [x] 利用イベントから収益分配集計が再現できるテストを追加する。
- CIタスク
  - [x] Marketplace bundleの検証ジョブを必須化する。
  - [x] Revenue Share集計回帰テストを必須化する。
- Exit Criteria
  - [x] Domain Pack公開に必要な最小機能が実装されている。

### PR-18: Advanced E2E Verification & Security Audit
- Status: `DONE`
- Spec Ref: SEC-02, AUD-01, ACT-01
- Dependencies: PR-11, PR-10, PR-03
- 実装タスク
  - [x] `core-runtime/tests/session_management.rs` の実装（ドメイン跨ぎセッション保存/復元、鍵ローテーション E2E）。
  - [x] `core-runtime/tests/audit_logging.rs` の実装（操作シーケンス一貫性、PIIマスク実効性）。
  - [x] 複雑な SPA 遷移における `stable_key` 自己修復のストレステストの実装。
- テストタスク
  - [x] `TOOL_CALL` および `STATE_SNAPSHOT` の両方で PII がマスクされていることを確認。
  - [x] 鍵ローテーション後、新しい鍵で旧セッションデータが正しく復号・再利用できることを確認。
- CIタスク
  - [x] 新規 E2E テストを `cdp-smoke` または `full-e2e` ジョブに追加。
- Exit Criteria
  - [x] 実機環境（Headless Chrome）において、仕様通りのセキュリティ・監査・リカバリが保証されている。

### PR-19: Comprehensive Evaluation Bench Expansion
- Status: `DONE` (Local)
- Spec Ref: Section 2.2, Section 5.2, Section 6, Section 7.1〜7.3
- Dependencies: PR-08, PR-10, PR-14, PR-16, PR-17, PR-18
- 実装タスク
  - [x] `smoke` / `full` の2モードで共通利用できる評価結果スキーマと artifact writer を整備する。
  - [x] `core-runtime` の代表シナリオ（semantic state, stable_key recovery, semantic wait, policy/HITL, audit redaction, session vault）を横断評価するベンチを追加する。
  - [x] `mcp-server` の代表シナリオ（tool contract, HITL flow, usage metering / plan gating）を横断評価するベンチを追加する。
  - [x] `skills-engine` / `plugin-host` / `marketplace` が同一フォーマットで結果を出力できる評価フックを追加する。
  - [x] 集約 JSON から Markdown ダッシュボードを生成するスクリプトと保存先規約を追加する。
- テストタスク
  - [x] `smoke` モードで必須シナリオ群がすべて評価対象に含まれることを失敗テストで固定化する。
  - [x] `full` モードで crate 横断の結果集約と failure / warning 反映が行われることを検証する。
  - [x] 主要な回帰（stable_key fallback, policy approval, audit masking, session restore, MCP ask_human）を評価ダッシュボード経由でも追跡できることを検証する。
- CIタスク
  - [x] PR 用の `evaluation-bench-smoke` ジョブを追加し、評価結果 JSON / Markdown を artifact として保存する。
  - [x] nightly / dispatch 用の `evaluation-bench-full` ジョブを追加し、全 crate の評価結果とダッシュボードを保存する。
  - [x] 必須シナリオ欠落または閾値超過時に CI を fail させるガードを追加する。
- Exit Criteria
  - [x] `dragon-head` の主要機能群を1つの評価ダッシュボードで横断確認できる。
  - [x] PR では短尺 smoke、nightly では full 評価が自動実行される。
  - [x] 追加された主要機能は評価ベンチへの登録なしでは完了扱いにできない運用ルールが `docs/` に明記されている。

> **Follow-up (ISSUE-154, done)**: Prompt-injection sanitizer v2 として、検出入力専用の
> HTML entity decode / NFKC / zero-width・control 文字除去 / common confusables mapping を追加。
> `PromptInjectionSanitizerConfig::additional_phrases` と `config.toml`
> `prompt_injection.additional_phrases` により追加リテラル phrase を指定可能にした。
> ReportOnly は元テキストを保持し、Redact は直接一致 phrase を部分置換、正規化後のみ一致する
> obfuscation は安全にフィールド全体を `[REDACTED_SECURITY]` へ置換する。
> stable_key は sanitizer 適用前生成の値を保持する回帰テストで固定した。

## 4. 共通 Definition of Done（全PR共通）

- [ ] 仕様トレーサビリティ（Spec Ref）がPR説明に記載されている。
- [ ] 最低1件の新規テストが追加されている。
- [ ] CI Required Checks が全て通過している。
- [ ] 破壊的変更がある場合、移行手順が `docs/` に追記されている。
