# AI_TASK_GUIDE.md

AIコーディングエージェントがこのリポジトリで作業する際のガイド。
最初に読む範囲を絞り、必要になった時点で追加の実装・テスト・仕様を開く。

## 最初に読むファイル

1. `docs/AI_CONTEXT.md` — リポジトリの地図。
2. `docs/ARCHITECTURE.md` — コンポーネント責務と主要データフロー。
3. `docs/FILE_GUIDE.md` — 機能別に見るべき実装・テストの対応表。
4. `GEMINI.md` — 追跡済みのエージェント向け概要とコマンド一覧。
5. ローカルの `AGENTS.md` / `CLAUDE.md` が存在する場合のみ読む。これらは
   `.gitignore` 対象なので、リポジトリ正本ではなく作業者固有の補助指示として扱う。
6. 対象機能に関係する実装ファイル。
7. 対象機能に関係するテストファイル。

`docs/SPEC.md` と `docs/PLAN.md` は必要になったときだけ参照する。
`PLAN.md` は履歴・計画の確認用であり、現在の実装状態はコードで再確認する。

## タスク別ガイド

### バグ修正

1. 再現条件を確認し、可能なら失敗するテストを先に作る。
2. `FILE_GUIDE.md` で関連する実装ファイルとテストファイルを特定する。
3. 同じ出力型を作る兄弟パスがないか `rg` で確認する。
4. 修正した関数・分岐を直接通るテストを追加する。
5. fallible な処理でメーター、audit、speculative state などを蓄積する場合は、
   エラー伝播の前に状態を保存しているか確認する。
6. `just check`、対象テスト、必要なら `just test-all` を実行する。

### 新規 MCP ツールまたはツール契約変更

1. `mcp-server/src/lib.rs` の既存 tool 定義、dispatch、input schema を確認する。
2. `mcp-server/tests/mcp_*` の契約テストを更新する。
3. `README.md` の Available MCP Tools、`docs/ARCHITECTURE.md`、
   `docs/AI_CONTEXT.md` を更新する。
4. Semantic State 構造を変える場合は `core-runtime/tests/sre_*` と
   `examples/` の fixture も確認する。

### Policy / HITL / Guardian Angel 変更

1. `core-runtime/src/policy.rs` と関連テストを読む。
2. JSON schema と MCP tool contract に影響があるか確認する。
3. HITL の挙動が変わる場合は `hitl-bridge/` と `docs/operations.md` を確認する。

### ドキュメント更新

1. 対象文書が参照している実装ファイルを開く。
2. 追跡されていない `AGENTS.md` / `CLAUDE.md` を正本として参照しない。
3. MCP tool 数、コマンド名、環境変数、crate 構成はコード・`Justfile`・
   `Cargo.toml` から確認する。
4. `python3 scripts/check_mvp_docs.py` を実行する。

## 注意領域

- `std::env::set_var` / `remove_var` を並列テストで使わない。
- `nfr-baseline/*.json` は `scripts/update_nfr_baseline.sh` 経由で更新する。
- `.config/nextest.toml` の `[profile.ci]` は `[profile.default]` を継承しない。
- `Cargo.lock` は手編集しない。
- LFS 管理のバイナリ fixture は、ドキュメント変更 PR に混ぜない。
