# AI_TASK_GUIDE.md

AIコーディングエージェントがこのリポジトリで作業する際のガイド。トークン
消費を抑えるため、まず最小限のファイルセットだけを読み、タスク種別ごとに
必要な範囲だけ追加で探索すること。

## 最初に読むファイル(最小セット)

1. `docs/AI_CONTEXT.md` — リポジトリの地図
2. `docs/ARCHITECTURE.md` — コンポーネント責務とデータフロー
3. `docs/FILE_GUIDE.md` — 「この機能を変えるならこのファイル」の対応表
4. `CLAUDE.md`(リポジトリルート) — 正本のクレート表とコマンド一覧、
   Rust固有の落とし穴(§1〜§14)
5. 対象機能に関係する実装ファイル(`FILE_GUIDE.md` の「機能別に見るべき
   場所」で特定する)
6. 対象機能に関係するテストファイル(同上)

`docs/SPEC.md` と `docs/PLAN.md` は必要になったときだけ参照する
(全文を毎回読む必要はない — `PLAN.md` はPRステータスの確認に使う)。

## タスク別ガイド

### バグ修正
1. 再現条件を確認する(可能ならテストで再現させる)。
2. `FILE_GUIDE.md` で関連する実装ファイルとテストファイルを特定する。
3. 同じファイル内に「兄弟パス」(同じ出力型を生成する他の関数/分岐)が
   ないか `grep` で確認する — CLAUDE.md のBugfix Discipline参照。
4. 最小範囲で修正する。`?` でエラーを伝播する箇所では、計測/監査用の
   状態がエラー伝播の前にキャプチャされているか確認する(CLAUDE.md §12)。
5. 修正対象のコードパスを直接アサートする回帰テストを追加する(集約した
   呼び出し元ではなく、直接の出力に対して)。
6. `just check` → 対象テスト → `just test-all` で確認。

### 新機能追加
1. `FILE_GUIDE.md`/`ARCHITECTURE.md` で既存の類似機能(似たツール、似た
   ポリシー拡張など)を探す。
2. 既存パターンに合わせる(例: 新しいMCPツールなら既存ツールの定義・
   ディスパッチ・契約テストの3点セットをコピーして変更)。
3. 影響範囲を確認する: MCPツール契約(`mcp-server/tests/mcp_*`)、Semantic
   Stateスキーマ(`core-runtime/tests/sre_*`, `stable_key_*`)、ポリシー
   スキーマ(`policy_schema_lint.rs`)、スキル/プラグインのスキーマ互換性
   テスト。
4. `docs/PLAN.md` に対応するPR/ISSUEがあれば状態を確認・更新する。
5. テストを追加し、`docs/testing.md` のテストピラミッド方針に従う。

### 設計変更・リファクタリング
1. `docs/ARCHITECTURE.md` の「Easy to break」セクションを必ず確認する。
2. 変更前後でテストが通ることを確認する(振る舞いを変えない場合)。
3. 監査ログ順序(audit→policy→execution)や `stable_key` 生成方式など、
   暗黙の契約を変えないこと。変える場合はユーザーに明示し、影響範囲
   (互換性テスト一覧)を提示する。

## テスト追加・更新の方針

- バグ修正のテストは修正対象のコードパスを直接アサートする(集約した
  呼び出し元経由のテストは弱い — CLAUDE.md「Test the Exact Path」)。
- ブラウザ依存テストは `test_bench_support::should_skip_browser_tests()`
  でスキップ可能にする。
- 環境変数を読むテストでは `std::env::set_var`/`remove_var` を使わない。
  関数を「envオプション引数を受け取る版」に分離し、テストはその引数に
  直接値を渡す(CLAUDE.md §11)。
- カバレッジ目標は80%(ユーザー個人ルール)。CIのカバレッジゲートは70%
  (`.github/workflows/ci.yml` の `coverage` ジョブ)。

## 既存設計を壊さないための注意

- `core-runtime` は他のワークスペースクレートに依存しない(基盤クレート)。
  この依存方向を逆転させる変更をしない。
- `act` 内の「監査ログ→ポリシー評価→実行」の順序を変えない。
- `SpeculativeEngine` のミスは必ず通常キャプチャにフォールバックする
  仕組み(`StateDelta::Mismatch`)を維持する — キャッシュヒットを無条件に
  信頼するコードを書かない。
- `nfr-baseline/*.json` は直接編集しない(`scripts/update_nfr_baseline.sh`
  経由)。
- `.config/nextest.toml` の `[profile.ci]` は `[profile.default]` を
  継承しないため、両方に必要なフィールドを明示的に複製する。

## やってはいけないこと

- 推測で `config.toml` の新フィールドを追加する(実際に
  `mcp-server/src/config.rs` の `FileConfig` 構造を確認してから)。
- `Cargo.lock` を手動編集する。
- `target/` 配下のファイルを参照・編集する(ビルド出力)。
- バイナリ/スナップショットフィクスチャ(`*.png`, `golden/*.json` 等)を
  意図した視覚差分更新以外の理由で変更する。
- CI/CD設定(`.github/workflows/*.yml`)を、`deny.toml` やnextestプロファイル
  との整合性を確認せずに変更する。
- 無関係なコードの整形・リファクタリング(ユーザーの依頼範囲外の変更)。

## 推測で変更してはいけない箇所

- `core-runtime/src/sre/stable_key.rs` のハッシュ/識別ロジック
  — 既存エージェント統合の要素識別が壊れる。
- `core-runtime/src/policy.rs` のenum/構造体のシリアライズ形式
  — `PolicyRule`/`PolicyDecision` のJSONスキーマはツール契約に含まれる。
- `deny.toml` のRUSTSEC例外リスト — 根拠なく削除/追加しない。
- 監査イベントのスキーマ(`audit_schema.rs` がテストする形式)。

## 大きな変更をする前に確認すべきこと

- `docs/PLAN.md` で同等の作業が既に計画/完了していないか。
- 変更対象のクレートに対応する `tests/` ディレクトリの一覧
  (`FILE_GUIDE.md` 参照)で、影響を受けるテストファイルを把握しているか。
- MCPツールのスキーマや`SemanticState`の構造を変える場合、
  `examples/mcp_examples/*.json` や `README.md` の表も更新が必要か。
- ユーザー個人ルール(`~/.claude/rules/`)の「Surgical Changes」原則
  ——変更は依頼内容に直接トレースできる範囲に留める。

## トークン削減のため、まず読むべき最小ファイルセット

| タスク種別 | 最小限読むファイル |
|---|---|
| バグ修正(範囲が明確) | `AI_CONTEXT.md` + 対象実装ファイル1〜2つ + 対応テスト |
| 新規MCPツール | `AI_CONTEXT.md` + `ARCHITECTURE.md`(データフロー節) + `mcp-server/src/lib.rs` + 既存ツールの契約テスト1例 |
| ポリシー/Guardian Angel変更 | `AI_CONTEXT.md` + `DOMAIN_MODEL.md`(該当エンティティ) + `core-runtime/src/policy.rs` + `policy_engine.rs`/`policy_enforcement.rs` |
| ドキュメント更新のみ | 対象ドキュメントと、その記述が参照する実装ファイルのみ(全文探索しない) |
