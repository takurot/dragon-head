# Dragon Head Improvement Discovery Prompt

このプロンプトは、gstack の skills を最大限使って `dragon-head` の新機能アイデア、改善案、技術的負債、検証ギャップ、プロダクト機会を洗い出すためのものです。目的は実装ではなく、次に投資すべき改善テーマを根拠付きで発見し、優先順位をつけることです。

## Prompt

あなたは `dragon-head`（Neural-Browser Runtime）の改善発見チームです。gstack skills を使って、プロダクト、技術設計、UI/UX、DX、QA、セキュリティ、リリース運用の観点から、このリポジトリの次の新機能候補と改善案を洗い出してください。

### 0. 作業モード

- 実装はしない。
- コード変更はしない。
- 必要なら読み取り専用のコマンドだけ実行する。
- 既存の未追跡ファイルや作業中の変更は尊重し、巻き戻さない。
- 改善案は必ずリポジトリ内の根拠に結びつける。
- 推測は「仮説」と明記し、確認方法も添える。

### 1. 初期コンテキスト収集

まず以下を読む。

- `README.md`
- `docs/SPEC.md`
- `docs/PLAN.md`
- `docs/PROMPT.md`
- `docs/testing.md`
- `docs/VERIFICATION.md` があれば読む
- `docs/REGISTERED_ISSUES.md` があれば読む
- `Cargo.toml`
- 各 workspace crate の `src` と `tests` の概要

次に、以下の観点で現在地を要約する。

- Dragon Head が解こうとしている主要課題
- 既に実装済みと見なされている機能
- 仕様上の強い約束、特に TTFT、Semantic State、Stable Key、Policy、Audit、Plugin、Skills、Marketplace
- 仕様と実装の間にありそうなズレ
- テストや検証で担保されていること、まだ弱そうなこと

### 2. gstack skill orchestration

以下の gstack skills を、必要に応じて順に使う。各 skill の結果はそのまま結論にしない。重複、矛盾、過剰スコープを統合して、最終的な改善バックログに落とす。

#### Product discovery

Use `gstack-office-hours`.

目的:
- Dragon Head の現在の価値仮説を問い直す。
- 「AI-native browser runtime」という表現の裏にある、本当に刺さるユーザー課題を具体化する。
- 最小 wedge と、より大きな事業機会を分ける。

出力:
- 想定ユーザー
- 最も痛い未解決課題
- 現在の仕様が強い領域
- 仕様にはあるが価値が弱い、または順番が早すぎる領域
- 新機能アイデア 5-10 個

#### Strategic product review

Use `gstack-plan-ceo-review`.

目的:
- 10-star product になり得る方向を探す。
- 単なる「runtime 機能追加」ではなく、ユーザーが明確に乗り換えたくなる体験を考える。
- Scope Expansion、Selective Expansion、Hold Scope、Reduction の4モードで候補を見る。

特に検討する問い:
- Dragon Head は library、MCP server、hosted runtime、enterprise control plane、marketplace のどれを最初に勝たせるべきか。
- Semantic State の差別化は、どの developer workflow で最も強く表れるか。
- 競合が Playwright / browser-use / browser automation MCP だとしたら、何が決定的に違うべきか。

#### Engineering architecture review

Use `gstack-plan-eng-review`.

目的:
- 現在の Rust workspace と仕様を照合し、次の設計投資を見つける。
- Core Runtime、MCP Server、Plugin Host、Skills Engine、Marketplace の境界を確認する。
- 高リスクな未実装、過小設計、テスト不足を抽出する。

重点領域:
- Semantic Rendering Engine の determinism と delta correctness
- Stable Key の互換性、衝突処理、DOM再レンダリング耐性
- Async pipeline の backpressure と failure handling
- Policy Engine と Audit Log の再現性
- Wasm Plugin sandbox と capability enforcement
- MCP protocol compatibility と schema evolution
- Skills Engine の verify -> policy_check -> act -> post_check 強制

#### DX review

Use `gstack-plan-devex-review` and `gstack-devex-review`.

目的:
- 新規開発者、plugin author、MCP client integrator、enterprise evaluator が迷う箇所を見つける。
- README、docs、examples、errors、test fixtures、CLI/API の使いやすさを評価する。

検討する改善例:
- `examples/` や quickstart の不足
- Semantic State fixture viewer
- MCP tool contract examples
- Plugin authoring template
- Policy rule cookbook
- Audit log replay tool
- Benchmark dashboard
- Troubleshooting guide

#### QA and verification

Use `gstack-qa-only`.

目的:
- 実装を変更せず、検証観点だけを洗い出す。
- テストの穴、flaky リスク、性能回帰リスク、CI必須化漏れを特定する。

確認対象:
- `cargo test --workspace` で担保される範囲
- integration / e2e / perf / security / schema compatibility の境界
- docs の完了表示と実際の検証証跡の整合
- fixture の代表性
- negative tests と abuse tests の不足

#### Security review

Use `gstack-cso`.

目的:
- AI agent が web を操作する runtime としての abuse path を洗い出す。
- Enterprise Security を機能名だけでなく、実際の trust boundary と攻撃経路で評価する。

重点脅威:
- prompt injection 経由の unsafe action
- policy bypass
- audit log tampering
- session vault leakage
- plugin sandbox escape
- malicious marketplace package
- credential exfiltration
- replay / race / TOCTOU
- cross-session data leakage

#### Browser and benchmark opportunities

Use `gstack-browse` and `gstack-benchmark` only if there is a runnable demo, local web target, docs site, or browser-visible artifact to inspect.

目的:
- 実際の developer-facing surface がある場合、体験と性能を観察する。
- ない場合は「browser-observable demo が不足している」という改善候補にする。

#### Release and operational maturity

Use `gstack-document-release`, `gstack-health`, and `gstack-retro` as read-only planning aids.

目的:
- docs と実装状態のズレを見つける。
- プロジェクト運用上、次に整えるべきレポート、ダッシュボード、リリース証跡を洗い出す。

### 3. Required final output

最終出力は日本語で、以下の構成にする。

## Executive Summary

- Dragon Head の現在の強み
- 最も大きい未回収の機会
- 次にやるべき改善テーマ上位3つ

## Improvement Backlog

表で出す。

| Rank | Theme | Type | User | Why now | Evidence | Proposed deliverable | Effort | Risk | Validation |
|---:|---|---|---|---|---|---|---|---|---|

Type は以下から選ぶ。

- Product
- Core Runtime
- MCP/API
- Plugin
- Skills
- Marketplace
- Security
- Performance
- DX
- QA
- Docs
- Ops

最低 15 件、できれば 25 件程度出す。

## Top 5 Deep Dives

上位5件について、各項目を詳述する。

- Problem
- User story
- Current repo evidence
- Proposed design
- Acceptance criteria
- Test strategy
- Security considerations
- Rollout plan
- What to explicitly not build yet

## Missing Evidence

判断に必要だが、現在の repo からは確認できなかった情報を列挙する。

## Suggested Next PRs

実装可能な PR 単位に分割する。

各 PR は以下を含める。

- PR title
- Scope
- Files likely touched
- Tests
- Exit criteria
- Dependencies

## gstack Skill Notes

使った gstack skills と、それぞれが何を発見したかを短く記録する。

### 4. Ranking rules

優先順位は次の順で評価する。

1. ユーザー価値が明確か
2. Dragon Head らしい差別化に直結するか
3. 既存アーキテクチャと自然に接続できるか
4. テスト可能か
5. セキュリティ・信頼性を悪化させないか
6. MVP後の採用・評価・販売に効くか

Effort は `S` / `M` / `L` / `XL`、Risk は `Low` / `Medium` / `High` で表す。

### 5. Quality bar

- 「便利そう」ではなく、誰がなぜ必要とするかを書く。
- 「AIでいい感じに」は禁止。入力、出力、失敗時挙動、検証方法を明記する。
- 既存仕様の焼き直しだけで終わらない。仕様にないが自然に伸ばせる案を含める。
- Security / Audit / Policy は必ず abuse case から考える。
- Performance 案は測定条件を必ず書く。
- DX 案は first 15 minutes experience を基準に評価する。
- Marketplace 案は supply side と demand side の両方を書く。

### 6. First prompt to run

以下をそのまま Codex に渡して開始する。

```text
Use the installed gstack skills to run an improvement discovery pass for this repository.

Do not implement or edit code. Read README.md, docs/SPEC.md, docs/PLAN.md, docs/PROMPT.md, docs/testing.md, docs/VERIFICATION.md if present, docs/REGISTERED_ISSUES.md if present, Cargo.toml, and the workspace crate layout.

Use gstack-office-hours, gstack-plan-ceo-review, gstack-plan-eng-review, gstack-plan-devex-review, gstack-devex-review, gstack-qa-only, gstack-cso, gstack-health, and gstack-retro as planning/review aids. Use gstack-browse and gstack-benchmark only if there is a runnable browser-facing target.

Produce a Japanese report with:
1. Executive Summary
2. Improvement Backlog with at least 15 ranked items
3. Top 5 Deep Dives
4. Missing Evidence
5. Suggested Next PRs
6. gstack Skill Notes

Tie every recommendation to repo evidence or mark it as a hypothesis with a validation method.
```
