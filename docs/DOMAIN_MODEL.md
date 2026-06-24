# DOMAIN_MODEL.md

## 用語集

| 用語 | 意味 | 補足 |
|---|---|---|
| Semantic State | ページのDOMをアクセシビリティツリー風に正規化したJSON表現 | エージェントが受け取る唯一のページ表現。生DOM/HTMLは渡さない |
| `stable_key` | `SemanticNode` の安定識別子(SHA-256ハッシュ+衝突インデックス) | 再描画後も同じ要素を指せるようにする |
| Semantic Delta | 2つの `SemanticState` 間の差分(RFC 6902 JSON Patch) | `get_state` のdelta取得モードで使用 |
| Load Profile | 状態キャプチャの解像度設定(`minimal`/`visual`/`interactive` 等) | 取得コストと情報量のトレードオフ |
| Policy Rule | URL/role/テキストにマッチする許可・禁止・承認要求の規則 | `Allow`/`Block`/`RequireHumanApproval` |
| Guardian Angel | ポリシールールに付与できる「結果予測」拡張機能 | 金額抽出+閾値で `Allow→Block/HITL` に動的昇格 |
| Outcome Projection | Guardian Angelが生成する予測結果(金額・リスクレベル) | 人間承認時に表示される |
| HITL (ask_human) | 人間によるアクション承認フロー | Slack/Teams経由(`hitl-bridge`) |
| Speculative State Generation | 次のアクションと結果状態を事前予測してキャッシュする仕組み | `get_state` のTTFTをほぼゼロにする |
| Skill | 宣言的なブラウザ操作ワークフロー定義(YAML/JSON) | Locate/Verify/Act/Wait/Extract/Handoffの固定ステップ語彙 |
| Plugin | Wasmで実装された外部拡張(状態観測・アクション前フック) | 署名・SBOM・capability検証が必須 |
| Audit Event | すべてのツール呼び出し・アクション・復旧イベントの構造化ログ | PII redaction済み、NDJSON/Webhookで永続化 |
| Security Flag | プロンプトインジェクション疑いを示すノード上のフラグ | `ReportOnly`/`Redact`/`Off` モードで挙動が変わる |
| Plan Tier | 課金プラン区分(`Developer`/`Pro`/`Enterprise`) | 使用量メータリングに影響 |

## 主要エンティティ

### SemanticState
- 役割: ある時点のページ全体を表すスナップショット。
- 主なフィールド: `page_instance_id`, `state_hash`, `timestamp`, `load_profile`, `root: SemanticNode`。
- 関連する処理: `PageSession::capture_semantic_state`、`select_update`(前状態との差分計算)。

### SemanticNode
- 役割: アクセシビリティツリーの再帰的な1ノード(ボタン、テキスト、リンク等)。
- 主なフィールド: `role`, `label`, `children`, `attributes`, `stable_key`, `ambiguous`, `alias`, `backend_node_id`, `security_flags`。
- 関連する処理: DOM正規化(`sre/normalization.rs`)、プロンプトインジェクション検出。

### SemanticDelta / StateUpdate
- 役割: 2つの `SemanticState` 間の差分転送。
- 主なフィールド: `previous_state_hash`, `next_state_hash`, `patch`(RFC 6902)。
- 状態: `StateUpdate` は `Noop{state_hash}` / `Full{state}` / `Delta{delta}` の3種。

### PolicyEngine / PolicyRule / PolicyDecision
- 役割: アクション実行前のガード。
- 主なフィールド(`PolicyRule`): `id`, `domain`, `path_prefix`, `role`, `text_regex`, `context_regex`, `action`, `scope`, `outcome_projector`。
- 状態: `PolicyAction` は `Allow` / `Block` / `RequireHumanApproval`。`ApprovalScope` は `ActionOnly` / `UntilNavigation` / `Timeboxed{ms}`。

### OutcomeProjection / RiskLevel
- 役割: Guardian Angelが生成する予測。
- 主なフィールド: `projected_amount: Option<f64>`, `risk_level`。
- 状態: `RiskLevel` は `Low` → `Medium` → `High` → `Critical`(金額閾値で段階的にエスカレーション)。

### SpeculativeEngine / SpeculativePrediction / StateDelta
- 役割: 次のアクションと結果状態をセッション履歴から予測しキャッシュ。
- 主なフィールド(`SpeculativePrediction`): `predicted_action`, `predicted_state_hash: Option<String>`, `confidence: f64`。
- 状態: `StateDelta::{Match, Mismatch}` — 予測が外れた場合は通常キャプチャにフォールバック。

### SkillDefinition / SkillStep
- 役割: 宣言的なブラウザ操作ワークフロー。
- 主なフィールド: `schema_version`, `name`, `steps: Vec<SkillStep>`。
- 状態: `SkillStep` は `Locate` / `Verify` / `Act` / `Wait` / `Extract` / `Handoff` の固定バリアント。実行結果は `SkillRunReport`/`OperationTrace`。

### PluginManifest / PluginRuntime
- 役割: Wasmプラグインの宣言と実行。
- 主なフィールド: `plugin_id`, `version`, `entry_points: Vec<ExtensionPoint>`, `capabilities: Vec<Capability>`, `signature`, `sbom`。
- 状態: `ExtensionPoint` は `OnState` / `BeforeAct` / `Connector`。`Capability` は `ReadState` / `NetworkOut` / `VaultAccess` でゲート。

### UsageReport / PlanTier
- 役割: MCPツール呼び出しの課金メータリング。
- 主なフィールド: `plan_tier`, `state_generations`(fast/full/delta/speculative), `visual_captures`, `actions_executed`, `hitl_events`, `audit_retention`, `cost_microusd`, `browser_restarts`, `speculative_misses`。

## エンティティ間の関係

```
PageSession 1--1 PolicyEngine
PageSession 1--1 SpeculativeEngine
PageSession 1--* SemanticState (時系列スナップショット)
SemanticState 1--* SemanticNode (rootからの再帰木)
SemanticState --diff--> SemanticDelta --> 次の SemanticState に適用可能
PolicyRule --has--> OutcomeProjectorConfig --produces--> OutcomeProjection
PolicyDecision --references--> PolicyRule, OutcomeProjection
SkillDefinition 1--* SkillStep --executed_by--> SkillEngine --drives--> PageSession
PluginManifest --validated_by--> plugin-host --hooks_into--> PageSession (plugin_hooks経由)
AuditLogger --records--> AuditEvent (ToolCall, BrowserRestart, ...) --persisted_by--> AuditSink
```

## ビジネスルール

- ポリシー評価より先にアクション試行を監査ログへ記録する(ブロックされた
  試行も追跡可能にする)。
- Guardian Angelの金額抽出は「最大値」を採用する。ページが小さい金額を
  先に表示して閾値検知を回避することを防ぐため。
- HITLイベントは `act` が `requires_human_approval` を返した時点と、
  `ask_human` が `approved=true` を返した時点の **2回** カウントされる
  (意図的な仕様。課金スペック §7.1)。
- 投機的予測がヒットした場合、その状態は「未検証」としてマークされ、次の
  `act` 実行前に実際のキャプチャで検証される。
- プロンプトインジェクション検出は `ReportOnly` では本文を変更せず
  `security_flags` のみ付与し、`Redact` では一致フレーズを
  `[REDACTED_SECURITY]` に置換する。

## 状態遷移

- **Chrome接続**: 正常 → (CDP切断検知 `is_browser_disconnected`) → 再起動
  (`BrowserClient::relaunch`)→ 新しい `PageSession` で復帰。
  `AuditEvent::BrowserRestart` を記録。
- **要素特定**: `stable_key` 一致 → 失敗時 `target_id` フォールバック →
  失敗時 `dom_signature` による自己修復照合 → 復旧したノードに対して
  ポリシーを再評価。
- **投機的状態**: 予測 → ヒット(未検証で即応答)/ミス(通常キャプチャ) →
  検証 → `Match`(確定)/`Mismatch`(ロールバック+ミスマッチログ記録)。

## 入力と出力

- **入力**: MCPクライアントからのJSON-RPCツール呼び出し(`get_state`,
  `act`, `verify`, `get_visual`, `ask_human`, `run_skill`,
  `get_usage_report`)、`config.toml`/環境変数、ポリシー/スキルJSONファイル。
- **出力**: `SemanticState`/`StateUpdate`(JSON)、アクション実行結果、
  監査ログ(NDJSON/Webhook)、使用量レポート(`UsageReport`)。

## データのライフサイクル

- `SemanticState` はセッション内でキャッシュされ、`act` 実行や明示的な
  `force_refresh` で無効化される。永続化はされない(プロセス内のみ)。
- 監査イベントは `AUDIT_LOG_DIR` 設定時のみディスクに永続化される
  (NDJSON、`AUDIT_LOG_MAX_BYTES` でローテーション)。未設定時は永続化なし。
- 投機的エンジンの `mismatch_log`(最大32件)と `snapshot_cache`
  (最大64件)はセッション内で古いものから自動的に破棄される(無制限増加
  を防ぐため)。
- セッション認証情報(クッキー等)は `SessionVault` に暗号化保存され、
  `BrowserClient` 単位で共有される。

## 開発時に誤解しやすい概念

- **Semantic Stateはアクセシビリティツリーに似ているが、CDPの
  Accessibilityドメインそのものではない** — 独自の正規化パイプライン
  (`sre/normalization.rs`)を経由する。
- **`stable_key` は要素の「位置」ではなく「内容ベースのハッシュ+衝突
  インデックス」** — DOM構造が変わっても同一要素なら基本的に維持される
  が、内容が変われば変化しうる。
- **投機的状態のヒットは「サーバー内の予測キャッシュ」であり、ブラウザ側を
  先行操作しているわけではない** — 実際のDOM操作は通常の `act` 経路でのみ
  発生する。
- **HITLの2重カウントは課金上の意図的仕様**であり、バグではない。
- **Guardian Angelはポリシールールの「付加機能」**であり、独立した別の
  承認エンジンではない(`PolicyRule.outcome_projector` として埋め込まれる)。
