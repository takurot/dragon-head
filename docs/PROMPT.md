# Neural-Browser Runtime: PR 実装プロンプト

このドキュメントは、`docs/PLAN.md` で定義された各PRを実装する際に、AIエージェントまたは開発者が従うべき詳細な実行手順書です。

## 1. 事前準備 (Context Setup)

開始する前に、以下の情報を確認し、セットアップを行います。

- **対象PR**: `docs/PLAN.md` から実装する PR ID (例: `PR-01`) を特定する。
- **仕様確認**: `docs/SPEC.md` の関連セクションと `docs/PLAN.md` の「実装タスク」「テストタスク」「Exit Criteria」を熟読する。
- **ブランチ作成**:
  ```bash
  git checkout main
  git pull
  git checkout -b feature/PR-XX-description
  ```

## 2. 実装サイクル (Implementation Cycle - TDD)

**厳格な TDD (Test-Driven Development)** サイクルを守って実装を進めます。

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

## 4. ドキュメント更新 (Documentation)

コード以外の成果物を同期します。

- **PLAN.md 更新**: 完了したタスクのチェックボックスを `[x]` に更新する。
- **SPEC.md 更新**: 実装中に仕様の微修正が必要になった場合、`SPEC.md` に反映する。
- **README/CHANGELOG**: 必要に応じて更新する。

## 5. PR作成と最終確認 (Finalization)

全てのチェックが完了したら、変更をプッシュしPRを作成します。

1.  **Commit**:
    - コミットメッセージは [Conventional Commits](https://www.conventionalcommits.org/) に従う。
    - 例: `feat(core): implement basic SRE logic (PR-02)`
2.  **Push**:
    ```bash
    git push origin feature/PR-XX-description
    ```
3.  **PR 作成**:
    - タイトル: `[PR-XX] 実装の概要`
    - 本文:
      - 関連Issue/Specへのリンク
      - `docs/PLAN.md` の「Exit Criteria」達成状況
      - 実施したテスト結果のスクリーンショットやログ
4.  **CI 確認**:
    - GitHub Actions のステータスを監視し、失敗した場合は即座に修正コミットを追加する。

---
**Note to AI Agent**: このプロンプトに従ってタスクを実行する際は、**「実装 → テスト → Lint/Format → Commit → Push → PR作成」までの工程を自律的に（ユーザ承認を挟まずに）実行すること**。
PR作成完了後、または解決不能なエラーが発生した場合のみユーザに報告し、レビューを依頼すること。
