# AI-Native Headless Browser Runtime 統合仕様書

- **Product Name**: Neural-Browser Runtime
- **Version**: 2.2 (Cathedral Edition)
- **Date**: 2026-05-03
- **Status**: Approved (Roadmap Expanded)

## 1. エグゼクティブサマリー

Neural-Browser Runtime は、LLM（大規模言語モデル）および VLM（視覚モデル）がWebを操作するために設計された、次世代のブラウザ実行環境（Runtime）である。従来のヘッドレスブラウザが「人間用DOMの操作自動化」に留まっていたのに対し、本製品はWebページを**「AIが解釈可能な意味的状態（Semantic State）」**へリアルタイムに変換するミドルウェアとして機能する。

### 1.1 コア・バリュープロポジション

- **Token Efficiency**: 独自の Semantic Rendering Engine (SRE) と差分更新により、LLMへの入力トークンを平均90%削減。
- **Reliability**: 視覚情報（SoM）と構造情報の同期、および stable_key による自己修復機能で、AIの誤操作（ハルシネーション）を防止。
- **Compliance**: Policy Engine と監査ログを標準搭載し、企業のセキュリティ要件を満たす「正規の代理実行環境」を提供する。
- **Speed**: 3段階のパイプライン処理、不要リソースのブロック、および**未来予測パイプライン**により、AIの「思考開始までの待ち時間（Near-Zero TTFT）」を実現。
- **Resilience**: **セマンティック・ヒーリング**により、UI変更に対する耐性を 99.9% まで向上。

## 2. システムアーキテクチャ

システムは「3層のレイヤー構造」と、Core Runtime内部の「4本の非同期パイプライン」で構成される。

### 2.1 Layer Structure

- **Layer 1: Core Runtime (Rust/C++)**
  Chromium CDP (Chrome DevTools Protocol) をラップし、レンダリング、SRE変換、セキュリティ制御を行う基盤。
- **Layer 2: Plugin Framework (Wasm Sandbox)**
  特定のサイトや業務に合わせてState抽出（Deep Lens）やポリシー（Guardian Angel）を拡張するサンドボックス環境。
- **Layer 3: Skills Engine**
  「商品購入」「求人検索」などのタスクを定義した宣言的ワークフローの実行エンジン。

### 2.2 Execution Pipeline (Internal)

並列性と応答性を最大化するため、Layer 1内部を非同期分離する。

- **Speculative Queue**: AIの意図を確率的に予測し、次遷移のSREを先行生成。
- **Render Queue**: DOM更新、レイアウト、必要最小限のペイント。
- **SRE Queue**: DOM解析、Stable Key生成、差分計算。
- **Audit/Policy Queue**: アクション判定、ログ暗号化・保存。

## 3. 詳細機能要件 (Core Runtime)

### 3.1 Semantic Rendering Engine (SRE)
WebページをAI用の構造化データに変換する中核エンジン。

**SRE-01: Deterministic State Generation**
- **入力**: Raw DOM, Computed Styles, Load Profile
- **Load Profile定義**:
  - `minimal`: テキスト/構造のみ。画像/動画/広告/解析JSをブロック（デフォルト・最速）。
  - `visual`: SoM生成時のみ画像リソースをロード。
  - `interactive`: ログインや複雑なSPA用。必須JSのみ許可。
- **正規化処理**:
  - 動的クラス名（ハッシュ値）の削除。
  - 広告 (`role="presentation"` 等) の除外。
  - **Fast State**: `interactive_elements` + `messages` のみを50ms以内に生成。
  - **Full State**: `forms`, `regions` を含む完全版をバックグラウンド生成。
- **出力**: Semantic State JSON（`page_instance_id`, `state_hash` を含む）。

**SRE-02: Semantic Delta (JSON Patch)**
- **仕様**: 前回の `state_hash` と比較し、変更差分（RFC 6902形式）のみを生成。
- **Subtree Refinement**: MutationObserver で検知した変更ノードとその親要素のみを再解析し、計算コストを最小化。

### 3.2 Native Set-of-Mark (SoM) & Stable Identity
VLMとLLMの認識を一致させ、要素特定を堅牢にする。

**ACT-01: Stable Key Generation**
- **課題**: 再レンダリングによる id（連番）の変化。
- **定義**: `stable_key` は SHA-256ハッシュのHex文字列 とする。人間可読な識別子は `alias` フィールドに分離する。
- **ロジック**: `sha256(role + normalized_label + dom_signature + quadrant)`
- **衝突解決**: 同一ページ内での衝突時はインデックスを付与し、`ambiguous: true` フラグを立てる。
- **Index**: `stable_key` と DOMノードの対応テーブルをメモリに常駐させ、探索を高速化。

**ACT-02: Event-Driven SoM**
- **仕様**: 常時生成を廃止。以下のトリガー時のみ画像生成パイプラインを動かす。
  - AIが `get_visual()` を明示的に要求した時。
  - `act()` が ambiguous エラーを返した時。
  - `verify()` が失敗した時。
- **Output**: 画像データに加え、`marks` メタデータ（ID, BBox, Stable Keyの対応表）を返す。

### 3.3 Interaction & Latency Optimization

**ACT-03: Semantic Wait**
従来の sleep を廃止し、SREからのイベント駆動で待機する。
- `wait_for_semantic(target="btn_login", state="enabled")`
- `wait_for_intent(intent="checkout_complete")`

**ACT-04: Robust Action Execution**
- **引数**: `target_id` (推奨) または `target_stable_key` (Fallback)。
- **Recovery Flow**:
  1. `target_id` で探索 → 失敗。
  2. `target_stable_key` で探索 → ヒットすれば実行し、Warningログ出力。
  3. 両方失敗 → **Self-Healing Layer** による修復を試行。
  4. 修復失敗 → `verify` 要求（または自動 `ask_human` フォールバック）を返す。

### 3.4 Enterprise Security (Policy & Audit)

**SEC-01: Context-Aware Policy Engine**
- **機能**: アクション実行前にルールベースの審査を行う。
- **評価基準**: ドメイン、パス、要素Role、テキスト（正規表現）、周辺テキスト（金額等）。
- **Action**: `allow`, `block`, `require_human_approval`.
- **Approval Scope**: `action_only` | `until_navigation` | `timeboxed(ms)` を指定可能。

**AUD-01: Structured Audit Log**
- **仕様**: 再現・追跡可能なイベントスキーマを定義する。
- **Event Model**:
  - `STATE_SNAPSHOT`: Full State (初回/Navigation時)
  - `STATE_PATCH`: RFC 6902 Delta (操作時)
  - `TOOL_CALL`: AIからの操作要求 (Act, Verify, Skill)
  - `POLICY_DECISION`: 審査結果 (Rule ID, Decision)
  - `HITL_EVENT`: 人間介入 (Request, Resume, User ID)
  - `VISUAL_CAPTURE`: SoM画像参照 (Hash)
- **PII Redaction**: `input[type="password/email"]` およびクレジットカード番号形式をデフォルトでマスク保存。

**SEC-02: Session Vault & Key Management**
- **機能**: 指定ドメインの認証情報（Cookie/Token）をAES-256で暗号化永続化。
- **Key Management**: デフォルトはプラットフォーム管理鍵。Enterpriseプランでは BYOK (Customer KMS) をサポートし、鍵ローテーションを強制する。

**SEC-03: Prompt Injection Sanitization**
- **目的**: Webページ内の不信頼テキストに含まれる、LLMへの間接命令やシステム指示の奪取を狙う既知パターンを検出し、SRE/MCP利用者へ構造化されたリスク情報として公開する。これは完全な遮断ではなく、defense-in-depth の一層として扱う。
- **対象**: SRE出力に含まれるすべてのLLM可視文字列（`label`, `alias`, `attributes` の文字列値、および子ノード配下のテキスト）。DOM本文だけでなく、`aria-label`, `title`, `placeholder`, `value` などの属性も対象とする。
- **Mode**: `Off` | `ReportOnly` | `Redact`。
  - `ReportOnly` (default): テキストは変更せず、検出されたノードへ `security_flags: ["possible_prompt_injection"]` を付与する。
  - `Redact`: 検出箇所を `[REDACTED_SECURITY]` に置換し、同じ `security_flags` を付与する。
  - `Off`: 検出・置換を行わない。
- **Stable Identity**: Sanitization は `stable_key` 生成後の SemanticNode tree に適用する。危険文言の置換によって stable key の衝突や不安定化を増やしてはならない。
- **Limitations**: v1は固定パターンによる既知リスクの検出に限定する。ユーザー定義regex、多言語網羅、ML分類器、Unicode/HTML entity難読化の完全対応は後続拡張とする。

### 3.5 Speculative State Generation (未来予測)
AIの思考レイテンシを排除するための先行実行エンジン。
- **予測ロジック**: セッション履歴とドメイン知識（Domain Pack）に基づき、現在のアクション後の次遷移を確率的に予測。
- **バックグラウンド生成**: 予測された次状態の SRE を先行生成し、AIの要求に対し Near-Zero TTFT でレスポンス。
- **バックトラッキング**: 予測が外れた場合、即座に `StateDelta::Mismatch` を返し、Full State 再送へフォールバックする。

### 3.6 Self-Healing Context Recovery (自己修復)
UI変更に対する耐性を極限まで高めるレジリエンス・レイヤー。
- **DOM Signature Cache**: 過去の成功した操作時のDOM構造（周辺ノード、属性、CSSパス）を署名としてキャッシュ。
- **修復ロジック**: `stable_key` が不一致の場合、キャッシュされた署名と現在のDOMをファジーマッチングし、最適なターゲットを再特定。
- **学習**: 修復成功時、新しい署名でキャッシュを更新。

### 3.7 "Guardian Angel" & Outcome Projection (プロアクティブ防御)
安全性を「大胆な行動の保証」に変える高度なセキュリティ。
- **Outcome Projection**: アクション実行前に、予測される副作用（決済額、在庫変動、予算消費等）を JSON 構造体として生成。
- **プロアクティブ防御**: 副作用がポリシーの閾値を超える場合、実行を自動ブロックし、人間（HITL）へ「未来の投影データ」付きで承認を求める。

### 3.8 Unified PII Redactor (統合プライバシーフィルター)
- **仕様**: SRE 出力と Audit Log 出力の両方に適用される、回避不能な強制フック。
- **対象**: `password/email` フィールド、クレジットカード形式、およびドメイン固有の機密パターン。

## 4. エコシステム仕様 (Extensions)

### 4.1 Plugin Framework (Wasm)

**PLUG-01: Extension Points**
- **State Plugin**: `on_state(json)` - 特定サイトの独自要素抽出（Lens機能）。
- **Policy Plugin**: `before_act(intent)` - 業界固有コンプライアンスルールの注入。
- **Connector Plugin**: 外部システム（SIEM, Slack）への通知。

**PLUG-02: Sandbox Security & Capabilities**
- **Capabilities**: マニフェストで権限を宣言（例: `read_state`, `network_out`, `vault_access`）。
- **Signature**: 署名済みPluginのみロード可能。SBOMによる依存管理を必須とする。
- **Wasm Runtime Hardening**: `wasmtime::Linker` のプーリングと **Epoch-based Interruption** を導入。1つのプラグインが暴走してもシステム全体を止めない隔離を実現。
- **Shared Engine & Caching**: インスタンス起動コストを最小化するため、コンパイル済みモジュールをキャッシュ。

### 4.2 Skills Layer (Declarative Workflows)

**SKILL-01: Skill Definition**
「検証可能なタスク」をJSONで記述したワークフロー。
- **構成要素**: `locate`, `verify`, `act`, `wait`, `extract`, `handoff`.
- **Execution Flow**: `verify` -> `policy_check` -> `act` -> `post_check` の順序を強制する。

### 4.3 "Deep Lens" Zero-Code Extraction DSL
AI とブラウザの間の会話を「操作」から「情報の取得」へとレベルアップさせる抽出エンジン。
- **仕様**: YAML/JSON ベースの抽出定義（例: `items: { selector: "tr.product", fields: { price: ".amt" } }`）。
- **Schema Registry**: DSL ルールをプリコンパイルし、実行時のパース・オーバーヘッドを排除。
- **Golden Dataset**: 正解データセット（`core-runtime/tests/fixtures/golden/`）による抽出精度の継続的自動評価。

## 5. API & Schema Definitions

### 5.1 Semantic State Schema (JSON)

```json
{
  "metadata": {
    "url": "https://example.com/checkout",
    "page_instance_id": "550e8400-e29b-...",
    "state_hash": "a1b2c3d4...",
    "load_profile": "minimal",
    "timestamp": 1707530000
  },
  "interactive_elements": [
    {
      "id": 42,                            // Session-scoped Integer
      "stable_key": "a1b2c3d4e5...",       // SHA-256 Hex String (Immutable)
      "alias": "btn_submit_checkout",      // Human-readable hint
      "role": "button",
      "name": "Purchase",
      "attributes": { "disabled": false },
      "bbox": [100, 200, 150, 50],
      "policy_flags": ["financial_transaction"],
      "security_flags": ["possible_prompt_injection"]
    }
  ]
}
```

### 5.2 MCP Tool Interface

Model Context Protocol (MCP) 準拠のツール定義。

| Tool Name | Arguments | Description |
| :--- | :--- | :--- |
| `get_state` | `format`: "json"\|"markdown", `force_refresh`: bool | ページ状態の取得。 |
| `get_state` output | `security_flags`: string[] | Prompt Injection Sanitization が検出した既知リスクを要素単位で返す。 |
| `act` | `target_id`: int, `target_stable_key`: string, `action`: "click"\|"type", `value`: string | アクション実行。 |
| `verify` | `target_id`: int, `expected`: {text: string} | ハルシネーション防止の事前検証。 |
| `get_visual` | `mode`: "clean"\|"som", `viewport`: "full" | 視覚情報の取得。 |
| `ask_human` | `reason`: string, `context`: bool, `outcome_projection`: object | HITL要求（2FA/判断不能/高額決済時）。承認要求に未来投影データを同梱。 |
| `run_skill` | `skill_name`: string, `params`: object | 定義済みSkillの実行。 |

**ACT-05: HITL Concurrency & Safety**
- **Session Lock**: Slack/Teams 等のチャットツール連携において、複数人による同時承認を防ぐ排他ロック機構。
- **Audit Trace**: 承認・却下を行ったユーザー ID、タイムスタンプ、およびその際の `Outcome Projection` データを不変ログに記録。

## 6. 非機能要件 (NFR) - 測定条件の明文化

**Performance Targets (Preconditions Defined):**

- **Near-Zero TTFT**: < 10ms (Speculative Hit)
- **State Update Latency**: < 100ms
  - Condition: Subtree Refinement有効, DOM変更ノード数 < 50。
- **Bandwidth**: 95%削減
  - Condition: 対Standard Profile比, 広告/画像ブロック有効時 (Minimal Profile)。

**Scalability:**

- **Capacity**: 75 Concurrent Sessions / Instance (2vCPU, 4GB RAM)
  - Definition: Minimal Profile適用, 画像生成なし, アイドル時のメモリ消費を含む。
  - Note: Visual Profile (画像生成あり) 利用時は 20 Sessions/Instance とする。

**Security:**

- PIIのログ出力はデフォルトマスク。
- Prompt Injection Sanitization はデフォルト `ReportOnly` で動作し、`security_flags` によって既知リスクを構造化して公開する。`Redact` mode はLLM可視テキストの一部を置換するが、完全なプロンプトインジェクション防御を保証するものではない。
- Wasmプラグインは分離メモリ空間で実行。

## 7. 付録: 収益化とプラン設計 (Monetization)

### 7.1 Billing Meters (Value-Based)

- **State Generations**: Fast/Full/Delta/Speculative生成回数を個別に計測。
- **Visual Captures**: SoM生成枚数（VLM認識価値）。
- **Actions Executed**: `act` の 成功回数（Attemptとは区別）。
- **HITL Events**: 人間への委譲発生回数（信頼性価値）。
- **Audit Retention**: ログ保持期間・容量（コンプライアンス価値）。

### 7.2 Plan Tiering

- **Developer (Free/PAYG)**: 基本Runtime, 標準Skills, ローカルログ。
- **Pro (Usage-based)**: Semantic Delta, SoM, 拡張プラグイン, Session Vault (Basic)。
- **Enterprise (Contract)**: Policy Engine (Full), Audit SIEM連携, SLA保証, Private Plugin Registry, BYOK。

### 7.3 Marketplace

- **Domain Packs**: 特定業務（経理、人事等）向けPlugin/Skillセットの販売。
- **Revenue Share**: 認定Plugin開発者への収益分配。

## 8. ツール & ユーティリティ

### 8.1 Side-by-side ROI Comparison Tool
- **機能**: 従来のブラウザ操作（Playwright等）と Dragon Head SRE を並行実行し、トークン消費量・レイテンシ・成功率の差分を定量評価する CLI ツール。
- **目的**: 導入によるコスト削減効果（ROI）をビジネス層へ客観的に証明する。
