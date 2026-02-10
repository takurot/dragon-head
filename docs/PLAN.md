# Neural-Browser Runtime 実装計画（進捗管理版）

- 対象仕様: [SPEC.md](./SPEC.md) v2.1（2026-02-10）
- 最終更新日: 2026-02-10
- プラン状態: Ready for Implementation

## 1. 進捗管理ルール

- `Status` は `NOT_STARTED` / `IN_PROGRESS` / `BLOCKED` / `DONE` を使用する。
- 各PRは `実装` / `テスト` / `CI` の3カテゴリでチェックボックス管理する。
- PRを `DONE` に変更できる条件:
  - 実装タスク完了
  - テストタスク完了
  - CIタスク完了
  - 受け入れ条件（Exit Criteria）を満たす
- 進捗率は `完了チェック数 / 全チェック数` で算出する。

## 2. フェーズダッシュボード

| Phase | Scope | PRs | Progress | Status |
| :--- | :--- | :--- | :--- | :--- |
| 0 | テスト・CI基盤 | PR-00 | 0/1 | NOT_STARTED |
| 1 | Core Runtime + SRE基盤 | PR-01〜03 | 0/3 | NOT_STARTED |
| 2 | Interaction & Reliability | PR-04〜06 | 0/3 | NOT_STARTED |
| 3 | Performance & NFR | PR-07〜08, PR-15 | 0/3 | NOT_STARTED |
| 4 | Security & Audit | PR-09〜11 | 0/3 | NOT_STARTED |
| 5 | Extensions & API | PR-12〜14 | 0/3 | NOT_STARTED |
| 6 | Monetization Meters | PR-16 | 0/1 | NOT_STARTED |
| 7 | Marketplace | PR-17 | 0/1 | NOT_STARTED |

## 3. PRバックログ（進捗チェック付き）

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
- Status: `NOT_STARTED`
- Spec Ref: SRE-01, Section 5.1
- Dependencies: PR-01
- 実装タスク
  - [ ] `minimal` / `visual` / `interactive` のLoad Profileを実装する。
  - [ ] 正規化処理（動的クラス除去、広告除外）を実装する。
  - [ ] `Fast State`（`interactive_elements`, `messages`）を優先生成する。
  - [ ] `Full State`（`forms`, `regions`）をバックグラウンド生成する。
  - [ ] `metadata` に `page_instance_id`, `state_hash`, `timestamp`, `load_profile` を含める。
- テストタスク
  - [ ] fixture HTMLに対するdeterministic出力テストを追加する。
  - [ ] Profile別のリソース制御（ブロック/許可）テストを追加する。
  - [ ] Fast/Full Stateの内容差分と生成順序のテストを追加する。
- CIタスク
  - [ ] SRE fixture回帰テストをCI必須ジョブ化する。
  - [ ] 仕様変更時のスナップショット更新をPR内で強制するチェックを追加する。
- Exit Criteria
  - [ ] 同一入力に対し `state_hash` が再現性を持つ。

### PR-03: ACT-01 Stable Key Generation
- Status: `NOT_STARTED`
- Spec Ref: ACT-01, Section 5.1
- Dependencies: PR-02
- 実装タスク
  - [ ] `sha256(role + normalized_label + dom_signature + quadrant)` を実装する。
  - [ ] `stable_key`（不変ID）と `alias`（人間可読名）を分離する。
  - [ ] 衝突時のインデックス付与と `ambiguous: true` を実装する。
  - [ ] `stable_key -> Node` インデックスをメモリ常駐化する。
- テストタスク
  - [ ] DOM再レンダリング時のキー安定性テストを追加する。
  - [ ] 衝突ケースで `ambiguous` が正しく立つことを検証する。
- CIタスク
  - [ ] stable key回帰テストをCI必須化する。
  - [ ] ハッシュ計算ロジックの変更時に互換性テストを必須化する。
- Exit Criteria
  - [ ] fallback探索に必要な `stable_key` インデックスが常に利用可能。

### PR-04: ACT-04 Robust Action Execution
- Status: `NOT_STARTED`
- Spec Ref: ACT-04
- Dependencies: PR-03
- 実装タスク
  - [ ] `act`（`click`, `type`）を `target_id` 優先で実装する。
  - [ ] `target_id` 失敗時に `target_stable_key` fallbackを実装する。
  - [ ] fallback成功時にWarningログを出力する。
  - [ ] 両方失敗時に `verify` 要求を返すフローを実装する。
- テストタスク
  - [ ] `target_id` 無効化時に `stable_key` で復旧するintegration testを追加する。
  - [ ] 二重失敗時に `verify required` を返すことを確認する。
- CIタスク
  - [ ] action回帰テストをPRごとに実行するジョブを追加する。
  - [ ] 失敗ケースのログ構造（warning/error）検証を自動化する。
- Exit Criteria
  - [ ] ACT-04 Recovery Flow（1→2→3）をテストで再現できる。

### PR-05: ACT-03 Semantic Wait
- Status: `NOT_STARTED`
- Spec Ref: ACT-03
- Dependencies: PR-04
- 実装タスク
  - [ ] `wait_for_semantic(target, state)` を実装する。
  - [ ] `wait_for_intent(intent)` を実装する。
  - [ ] SRE Queueイベントを購読して待機解除を行う。
- テストタスク
  - [ ] 遅延ロードページで `enabled` 待機が機能するE2Eを追加する。
  - [ ] intent成立/不成立のタイムアウト挙動テストを追加する。
- CIタスク
  - [ ] semantic wait E2Eをsmoke対象に含める。
  - [ ] タイムアウトが閾値を超える場合に失敗する性能アサーションを追加する。
- Exit Criteria
  - [ ] 固定sleepに依存しない待機APIが利用可能。

### PR-06: ACT-02 Event-Driven SoM
- Status: `NOT_STARTED`
- Spec Ref: ACT-02
- Dependencies: PR-02, PR-04
- 実装タスク
  - [ ] 常時SoM生成を廃止し、イベント駆動パイプラインに切り替える。
  - [ ] トリガー条件（`get_visual`, `act` ambiguous, `verify` failure）を実装する。
  - [ ] `marks`（ID/BBox/stable_key対応）付き出力を実装する。
- テストタスク
  - [ ] 非トリガー時にSoMが生成されないことを検証する。
  - [ ] トリガー3種ごとに `marks` 整合性を検証する。
- CIタスク
  - [ ] SoM生成テストでスクリーンショット差分をartifact保存する。
  - [ ] 画像差分しきい値超過時に失敗するチェックを追加する。
- Exit Criteria
  - [ ] SoM生成回数がイベント発火時に限定される。

### PR-07: SRE-02 Semantic Delta (RFC 6902)
- Status: `NOT_STARTED`
- Spec Ref: SRE-02
- Dependencies: PR-02
- 実装タスク
  - [ ] `state_hash` 差分に基づく JSON Patch（RFC 6902）生成を実装する。
  - [ ] MutationObserver連携で変更サブツリーのみ再解析する。
  - [ ] Full StateとDeltaの切り替えポリシーを実装する。
- テストタスク
  - [ ] 軽微DOM変更でpatchサイズが縮小することを検証する。
  - [ ] patch適用後の再構築Stateが原本一致することを検証する。
- CIタスク
  - [ ] RFC 6902準拠テストスイートをCI必須化する。
  - [ ] patchサイズ回帰（肥大化）検出チェックを追加する。
- Exit Criteria
  - [ ] Delta送信でフル再送を回避できる。

### PR-08: Async Pipeline Architecture
- Status: `NOT_STARTED`
- Spec Ref: Section 2.2, Section 6（TTFT）
- Dependencies: PR-07
- 実装タスク
  - [ ] `Render Queue`, `SRE Queue`, `Audit/Policy Queue` を非同期分離する。
  - [ ] `Fast State` 優先スケジューリングを実装する。
  - [ ] Queue間のバックプレッシャー制御を実装する。
- テストタスク
  - [ ] 高負荷下でもデッドロックしないことを検証する負荷テストを追加する。
  - [ ] TTFT計測テスト（Fast State条件）を追加する。
- CIタスク
  - [ ] PRでは短時間ベンチ、nightlyでは長時間ベンチを実行する。
  - [ ] TTFTの回帰を検知して警告/失敗にするしきい値を設定する。
- Exit Criteria
  - [ ] Queue分離後も機能退行がなく、TTFT目標達成の見込みが示せる。

### PR-09: SEC-01 Context-Aware Policy Engine
- Status: `NOT_STARTED`
- Spec Ref: SEC-01
- Dependencies: PR-04
- 実装タスク
  - [ ] Policyルール定義（domain/path/role/text/context）を実装する。
  - [ ] `allow` / `block` / `require_human_approval` の判定を実装する。
  - [ ] approval scope（`action_only`, `until_navigation`, `timeboxed(ms)`）を実装する。
- テストタスク
  - [ ] ルール条件ごとの判定テストを追加する。
  - [ ] scopeの有効期限・navigation跨ぎの挙動テストを追加する。
- CIタスク
  - [ ] policy regression suiteを必須ジョブ化する。
  - [ ] ルールファイルの静的検証（schema lint）を追加する。
- Exit Criteria
  - [ ] 重要アクションに対する事前審査が必ず実行される。

### PR-10: AUD-01 Structured Audit Log & PII Redaction
- Status: `NOT_STARTED`
- Spec Ref: AUD-01
- Dependencies: PR-08
- 実装タスク
  - [ ] Event Model（`STATE_SNAPSHOT`, `STATE_PATCH`, `TOOL_CALL`, `POLICY_DECISION`, `HITL_EVENT`, `VISUAL_CAPTURE`）を実装する。
  - [ ] 非同期ログ書き込みを実装する。
  - [ ] PIIマスク（`password/email` フィールド、クレジットカード形式）を実装する。
- テストタスク
  - [ ] 各イベントタイプのスキーマ検証テストを追加する。
  - [ ] 機密入力が必ずマスクされるテストを追加する。
- CIタスク
  - [ ] 監査ログスキーマ互換性チェックを必須化する。
  - [ ] PII漏洩検知テストを必須化する。
- Exit Criteria
  - [ ] 再現可能な監査トレースを出力できる。

### PR-11: SEC-02 Session Vault & Key Management
- Status: `NOT_STARTED`
- Spec Ref: SEC-02
- Dependencies: PR-01
- 実装タスク
  - [ ] Cookie/TokenのAES-256暗号化保存を実装する。
  - [ ] `SessionVault` 抽象とローカル実装を追加する。
  - [ ] BYOK対応インターフェース（KMSアダプタ）を設計する。
  - [ ] 鍵ローテーションAPIを実装する。
- テストタスク
  - [ ] 保存データが平文でないことを検証する。
  - [ ] 鍵ローテーション後の復号互換テストを追加する。
- CIタスク
  - [ ] mock KMSを用いたvault integration testを追加する。
  - [ ] 暗号関連依存の脆弱性スキャンをCIに追加する。
- Exit Criteria
  - [ ] Local鍵とBYOKの双方でセッション再利用が可能。

### PR-12: PLUG-01/02 Plugin Framework (Wasm)
- Status: `NOT_STARTED`
- Spec Ref: PLUG-01, PLUG-02
- Dependencies: PR-02, PR-09
- 実装タスク
  - [ ] Wasmランタイム（`wasmtime` 等）を導入する。
  - [ ] Extension Point（`on_state`, `before_act`, connector）を定義する。
  - [ ] capability manifest（`read_state`, `network_out`, `vault_access`）を実装する。
  - [ ] 署名済みpluginのみロード可能にする。
  - [ ] SBOM提出/検証フローを実装する。
- テストタスク
  - [ ] 非署名pluginが拒否されることを検証する。
  - [ ] capability逸脱アクセスが遮断されることを検証する。
- CIタスク
  - [ ] plugin署名検証ジョブを必須化する。
  - [ ] SBOM検証ジョブを必須化する。
- Exit Criteria
  - [ ] サンドボックス制約下で安全にplugin実行できる。

### PR-13: SKILL-01 Skills Engine
- Status: `NOT_STARTED`
- Spec Ref: SKILL-01
- Dependencies: PR-04, PR-09
- 実装タスク
  - [ ] Skill JSON schema（`locate`, `verify`, `act`, `wait`, `extract`, `handoff`）を定義する。
  - [ ] 実行順序 `verify -> policy_check -> act -> post_check` を強制する。
  - [ ] リトライ・分岐・handoff処理を実装する。
- テストタスク
  - [ ] 正常系ワークフロー（例: 検索〜抽出）を追加する。
  - [ ] `verify` 失敗時に `act` が抑止されるテストを追加する。
- CIタスク
  - [ ] skill conformance testを必須ジョブ化する。
  - [ ] schema変更時の後方互換性チェックを追加する。
- Exit Criteria
  - [ ] 宣言的Skillで再現性あるタスク実行が可能。

### PR-14: MCP Tool Interface
- Status: `NOT_STARTED`
- Spec Ref: Section 5.2
- Dependencies: PR-04, PR-05, PR-06, PR-09, PR-13
- 実装タスク
  - [ ] MCPサーバを実装し内部APIを公開する。
  - [ ] `get_state`, `act`, `verify`, `get_visual`, `ask_human`, `run_skill` を公開する。
  - [ ] 各tool引数・戻り値を仕様準拠に揃える（`force_refresh` など）。
- テストタスク
  - [ ] MCP clientとの契約テストを追加する。
  - [ ] `ask_human` を含むHITLフローのE2Eテストを追加する。
- CIタスク
  - [ ] MCP protocol complianceテストを必須化する。
  - [ ] API schema差分の自動検知を追加する。
- Exit Criteria
  - [ ] 仕様ツール群を外部クライアントから一貫利用できる。

### PR-15: NFR Benchmark & Capacity Validation
- Status: `NOT_STARTED`
- Spec Ref: Section 6
- Dependencies: PR-08, PR-10, PR-14
- 実装タスク
  - [ ] TTFT `<50ms`（Fast State, Minimal Profile条件）計測基盤を実装する。
  - [ ] State Update Latency `<100ms`（変更ノード < 50条件）を計測する。
  - [ ] 帯域95%削減（Standard比）を計測する。
  - [ ] 容量指標（Minimal 75 sessions/instance, Visual 20 sessions/instance）を測定する。
- テストタスク
  - [ ] 再現可能なベンチシナリオと負荷プロファイルを固定化する。
  - [ ] 結果の統計妥当性（複数試行、p95/p99）を検証する。
- CIタスク
  - [ ] PRでは短尺性能テスト、nightlyではフル性能テストを実行する。
  - [ ] NFR回帰ダッシュボードを生成し、しきい値超過時に失敗させる。
- Exit Criteria
  - [ ] 主要NFR指標が計測可能かつ継続監視可能。

### PR-16: Billing Meters & Plan Gating
- Status: `NOT_STARTED`
- Spec Ref: Section 7.1, 7.2
- Dependencies: PR-10, PR-14
- 実装タスク
  - [ ] Meter（`State Generations`, `Visual Captures`, `Actions Executed`, `HITL Events`, `Audit Retention`）を実装する。
  - [ ] プラン別feature gate（Developer/Pro/Enterprise）を実装する。
  - [ ] 利用量レポートAPIを実装する。
- テストタスク
  - [ ] メーター計測精度テストを追加する。
  - [ ] プラン境界のアクセス制御テストを追加する。
- CIタスク
  - [ ] メータリング回帰テストを必須化する。
  - [ ] 料金計算スナップショット差分チェックを追加する。
- Exit Criteria
  - [ ] 仕様の課金メーターが再現性を持って収集できる。

### PR-17: Marketplace Integration
- Status: `NOT_STARTED`
- Spec Ref: Section 7.3
- Dependencies: PR-12, PR-16
- 実装タスク
  - [ ] Domain Pack（plugin + skill bundle）のパッケージ仕様を定義する。
  - [ ] Marketplace向け公開メタデータ（署名、互換バージョン、依存情報）を実装する。
  - [ ] Revenue Share集計に必要な利用イベント連携を実装する。
- テストタスク
  - [ ] Bundleの署名検証・互換性検証テストを追加する。
  - [ ] 利用イベントから収益分配集計が再現できるテストを追加する。
- CIタスク
  - [ ] Marketplace bundleの検証ジョブを必須化する。
  - [ ] Revenue Share集計回帰テストを必須化する。
- Exit Criteria
  - [ ] Domain Pack公開に必要な最小機能が実装されている。

## 4. 共通 Definition of Done（全PR共通）

- [ ] 仕様トレーサビリティ（Spec Ref）がPR説明に記載されている。
- [ ] 最低1件の新規テストが追加されている。
- [ ] CI Required Checks が全て通過している。
- [ ] 破壊的変更がある場合、移行手順が `docs/` に追記されている。
