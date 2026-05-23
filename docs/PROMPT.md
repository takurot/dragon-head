# Neural-Browser Runtime: PR 実装プロンプト

このドキュメントは、`docs/PLAN.md` で定義された各PRを実装する際に、AIエージェントまたは開発者が従うべき詳細な実行手順書です。

## 1. 事前準備 (Context Setup)

### 1.0 入力パラメータ

このプロンプトは以下のパラメータを受け取ります。

| パラメータ | 必須 | 例 | 説明 |
|-----------|------|-----|------|
| `PLAN_TASK` | 必須 | `"Phase 2.1: SRE pipeline"` | `docs/PLAN.md` 上のタスク記述 |
| `ISSUE` | 任意 | `42` | GitHub Issue 番号。背景・要件・議論の追加コンテキストとして使う |

**`ISSUE` が指定された場合**、実装開始前に以下を実行して Issue の内容をコンテキストに取り込む:
```bash
gh issue view <ISSUE> --json title,body,comments,labels
```
Issue の要件・仕様・議論に基づき、`docs/PLAN.md` の Exit Criteria と照合した上で実装方針を確定する。

---

開始する前に、以下の情報を確認し、セットアップを行います。

- **対象PR**: `docs/PLAN.md` から実装する PR ID (例: `PR-01`) を特定する。
- **仕様確認**: `docs/SPEC.md` の関連セクションと `docs/PLAN.md` の「実装タスク」「テストタスク」「Exit Criteria」を熟読する。
- **テスト戦略確認**: `docs/testing.md` を読み、実装するテストの種類と配置ルール（Unit/Integration/E2Eの使い分け）を確認する。
- **ブランチ作成** (`ISSUE` がある場合はブランチ名に含める):
  ```bash
  git checkout main
  git pull
  # ISSUE なし
  git checkout -b feature/PR-XX-description
  # ISSUE あり (例: issue/42-PR-XX-description)
  git checkout -b issue/<ISSUE>-PR-XX-description
  ```

## 2. 実装サイクル (Implementation Cycle - TDD)

**厳格な TDD (Test-Driven Development)** サイクルを守って実装を進めます。

> **Skills の活用**: 実装・テスト・レビュー・E2E 等の各フェーズで、タスクに適した Skills (`/tdd`, `/rust-test`, `/rust-review`, `/e2e`, `/code-review` など) を積極的に使うこと。汎用的な実装より Skill を使った方が品質・速度ともに高い。

1.  **Red (テスト作成)**:
    - `docs/PLAN.md` の「テストタスク」に基づき、失敗するテストケースを作成する。
    - `cargo test` (Rust) または `pytest` (Python) を実行し、**期待通りに失敗すること**を確認する。
2.  **Green (最小実装)**:
    - テストをパスさせるための最小限の実装を行う。
    - `cargo check` / `cargo build` が通ることを確認する。
    - テストを再実行し、パスすることを確認する。
3.  **Refactor (リファクタリング)**:
    - コードの可読性、構造、パフォーマンスを改善する。
    - 再度テストを実行し、破壊していないことを確認する。

## 3. 品質保証 (Quality Assurance)

実装完了後、PR作成前に以下のローカル検証を**必ず**実行します。

### 3.1 テスト全実行
- Unit Test, Integration Test, E2E Test (関連する場合) を全て実行する。
  ```bash
  cargo test --workspace
  # または
  just test-all  # (もし定義されていれば)
  ```

### 3.2 静的解析 (Lint & Format)
- コードスタイルと潜在的なバグを修正する。
  ```bash
  cargo fmt --all -- --check
  cargo clippy --workspace -- -D warnings
  ```
  - **重要**: Clipyの警告はすべて修正する（`-D warnings` でエラー化するため）。

### 3.3 回帰テスト・ベンチマーク (Regression & NFR)
- `docs/PLAN.md` の「CIタスク」や「Exit Criteria」に含まれる特定の検証項目（TTFT計測など）を手動で実行し、結果を記録する。

## 4. gstack Review → QA ゲート

ローカル検証が通った後、コミット前に gstack skills を使って **review → qa** の順で追加検証を行います。ここで見つかった問題は修正し、該当テストを追加または更新してから、再度 `3. 品質保証` へ戻ります。

### 4.1 Pre-landing Review
- `gstack-review` を使い、base branch との差分を pre-landing review する。
- 重点観点:
  - 仕様・`docs/PLAN.md` の Exit Criteria とのズレ
  - Rust workspace / crate 境界の崩れ
  - Policy / Audit / Session Vault / Plugin / MCP など trust boundary の破れ
  - テスト不足、回帰リスク、NFR への悪影響
  - 不要な scope creep や関連しない変更
- P1/P2 相当の指摘は必ず修正する。修正後は関連テストを再実行し、必要なら `gstack-review` を再実行する。

### 4.2 QA
- `gstack-qa` を使い、実装した機能をユーザー視点で QA する。
- runnable な browser-facing target、MCP server、demo、fixture viewer、examples がある場合:
  - 実際に起動して主要フローを操作する。
  - 失敗、表示崩れ、エラー応答、ログ、artifact を確認する。
  - `gstack-browse` / `gstack-benchmark` が適用できる場合は、`gstack-qa` の中で活用する。
- browser-facing target がない backend / library / schema-only 変更の場合:
  - `gstack-qa-only` 相当の report-only 観点で、contract test、negative test、schema compatibility、fixture coverage、CI gate 漏れを確認する。
  - 必要に応じて `gstack-health` で品質ゲートの抜けを確認する。
- QA で bug を見つけた場合:
  1. 最小修正を行う。
  2. 再現テストまたは回帰テストを追加する。
  3. `cargo test` / 対象テスト / lint を再実行する。
  4. `gstack-review` → `gstack-qa` を再実行し、未解決の重大指摘がないことを確認する。

### 4.3 記録
- PR本文に以下を記録する。
  - `gstack-review` の結果概要と未解決事項の有無
  - `gstack-qa` / `gstack-qa-only` の対象、実行観点、結果
  - 追加で実行した `gstack-browse` / `gstack-benchmark` / `gstack-health` があれば、その結果
  - 修正した review / QA 指摘と、それを担保するテスト

## 5. ドキュメント更新 (Documentation)

コード以外の成果物を同期します。

- **PLAN.md 更新**: 完了したタスクのチェックボックスを `[x]` に更新する。
- **SPEC.md 更新**: 実装中に仕様の微修正が必要になった場合、`SPEC.md` に反映する。
- **README/CHANGELOG**: 必要に応じて更新する。

## 6. PR作成と最終確認 (Finalization)

全てのチェックが完了したら、変更をプッシュしPRを作成します。

1.  **Commit**:
    - トラブルシューティング: 新規ファイル追加時は必ず `git add <file>` を忘れないこと（CIエラーの主因）。
    - コミットメッセージは [Conventional Commits](https://www.conventionalcommits.org/) に従う。
    - 例: `feat(core): implement basic SRE logic (PR-02)`
2.  **Push**:
    ```bash
    git push origin feature/PR-XX-description
    ```
3.  **PR 作成**:
    - タイトル: `[PR-XX] 実装の概要` (`ISSUE` がある場合: `[ISSUE-<N>] 実装の概要`)
    - 本文:
      - 関連 Issue へのリンク (`Closes #<ISSUE>` を含める)
      - `docs/PLAN.md` の「Exit Criteria」達成状況
      - 実施したテスト結果のスクリーンショットやログ
      - `gstack-review` → `gstack-qa` の実行結果と、未解決事項がないこと
4.  **CI 確認**:
    - GitHub Actions のステータスを監視し、失敗した場合は即座に修正コミットを追加する。

## 7. Codex コードレビューとフィードバック対処 (Codex Review Loop)

PR 作成完了後、以下の手順で Codex レビューを実施します。

1. **Codex コードレビュー依頼**:
   - `/codex:rescue` または `codex:codex-review` を使い、作成した PR のコードを Codex にレビュー依頼する。
   - レビュー対象: 実装コード全体、テスト、PR diff。

2. **指摘の PR 投稿**:
   - Codex から返ってきたレビュー結果（P1/P2 相当の指摘）を、GitHub PR にインラインコメントまたはレビューコメントとして投稿する。
   - `gh pr review --comment` / `gh api` を使って投稿する。

3. **指摘への対処と CI 修正ループ**:
   - P1（重大）/ P2（重要）の指摘を優先して修正する。
   - 修正後は関連テストを再実行し、`cargo fmt` / `cargo clippy` を通す。
   - 修正コミットをプッシュし、CI (GitHub Actions) のステータスを確認する。
   - **CI がオールグリーンになるまでこのループを繰り返す**。
     ```bash
     # CI ステータス確認
     gh pr checks <PR番号>
     ```
   - 解決不能な P1 指摘がある場合のみユーザに報告する。

## 8. マージ判定 (Merge Gate)

以下の全条件を満たしたときのみマージを実行します。

- [ ] Codex レビューの P1/P2 指摘がすべて対処済み
- [ ] 全テスト (Unit / Integration / E2E) がパス
- [ ] CI (GitHub Actions) の全ジョブがオールグリーン
- [ ] `gstack-review` / `gstack-qa` で未解決の重大指摘なし
- [ ] `docs/PLAN.md` の Exit Criteria を達成

全条件を満たした場合:
```bash
gh pr merge <PR番号> --squash --delete-branch
```

## 9. セッション学習の保存 (Post-merge Learning)

マージ完了後、セッションで得た再利用可能な知見を `/learn` スキルで自動保存します。

### 9.1 非インタラクティブ実行

`/learn` をそのまま呼ぶとユーザー確認を求めるインタラクティブモードになります。エージェントからは **`--auto` 引数を渡して確認ステップをスキップ**します:

```
Skill("learn", args="--auto")
```

`--auto` を渡した場合のスキップ対象:
- Step 4「Ask user to confirm before saving」→ 確認なしで即保存

### 9.2 抽出対象の優先順位

このセッションで以下が発生していた場合に限り保存する（トリビアルな修正は除外）:

1. **エラー解決パターン** — ビルドエラー、テスト失敗、CI 失敗の根本原因と修正方法
2. **Rust / ツールチェーン固有の挙動** — cargo, clippy, nextest, llvm-cov 等の非自明な挙動
3. **ワークアラウンド** — ライブラリのバグ、API 制限、バージョン固有の問題
4. **プロジェクト固有パターン** — crate 境界の慣習、trust boundary の扱い、PR テンプレート規約

### 9.3 保存先と MEMORY.md 更新

`/learn --auto` の実行後、`~/.claude/skills/learned/<pattern-name>.md` に保存されたファイルを確認し、`MEMORY.md` のインデックスに1行追加する:

```bash
# 保存されたスキルを確認
ls -t ~/.claude/skills/learned/ | head -5
```

---
**Note to AI Agent**: このプロンプトに従ってタスクを実行する際は、**「(Issue 番号が指定された場合は `gh issue view` でコンテキスト取得) → ブランチ作成 → プランニング → TDD実装（適切なSkills使用）→ Lint/Format → gstack-review → gstack-qa → 必要な修正 → Commit → Push → PR作成（`Closes #<ISSUE>` を含む）→ Codex コードレビュー依頼 → 指摘をPRに投稿 → 指摘対処 → CI オールグリーン → マージ → `Skill("learn", args="--auto")` でセッション学習を保存」までの工程を自律的に（ユーザ承認を挟まずに）実行すること**。
解決不能なエラーが発生した場合、または P1 指摘が対処不能な場合のみユーザに報告し、レビューを依頼すること。
