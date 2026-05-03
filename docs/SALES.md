# Dragon Head: Neural-Browser Runtime 販売・ビジネス戦略

## 1. ビジョン
**"The Sensing and Acting Organ for every AI Agent."**
Dragon Head は、世界中の AI エージェントにとって最も信頼性が高く、低コストで、安全な「感覚器官（ブラウザ取得）」および「実行器官（ブラウザ操作）」の標準インフラを目指す。

---

## 2. ターゲット市場と顧客プロファイル (ICP)

### 2.1 セグメントA: AI エージェント・スタートアップ / 開発企業
*   **特性**: 自社で AI エージェント製品（経理自動化、リサーチ自動化など）を開発している企業。
*   **課題**: LLM のトークンコストが利益を圧迫している。Playwright 等の既存ツールでは UI 変更に弱く、保守コストが高い。

### 2.2 セグメントB: 大企業（エンタープライズ）の DX/RPA 推進部門
*   **特性**: 既存の RPA（UiPath, Blue Prism）が UI 変更で頻繁に壊れることに疲弊している。AI を導入したいがセキュリティ審査を通せない。
*   **課題**: 従来のオートメーションの「壊れやすさ」。AI の自律動作に対する「ガバナンスと恐怖」。

### 2.3 セグメントC: システムインテグレーター (SIer) / コンサルティング
*   **特性**: クライアント企業の業務自動化を受託開発している。
*   **課題**: 高品質で安定した AI オートメーションを短期間で構築する必要がある。

---

## 3. コア・バリュープロポジション (差別化要因)

1.  **コスト削減 (Token Efficiency)**:
    *   Semantic Delta により、従来の DOM 送信に比べトークン消費を 90% 以上削減。利益率の低いエージェントビジネスを「稼げるビジネス」に変える。
2.  **自己修復性 (Deterministic Stability)**:
    *   Stable Key 技術により、UI が頻繁に変わる SPA やモダンな Web サイトでも AI が要素を見失わない。
3.  **コンプライアンス保証 (Enterprise Ready)**:
    *   Policy Engine と HITL (Human-in-the-Loop) により、「AI が勝手に誤操作・誤購入するリスク」をゼロに抑える唯一のソリューション。

---

## 4. 製品ラインナップ構想

AI エージェント市場の成熟に合わせ、単なるランタイムを超えた 4 つの製品軸を展開する。

### 4.1 Dragon Head "Core" (エージェントの基盤)
AI エージェント開発者が「自分の製品」に組み込むためのミドルウェア。
*   **DH SDK / API**: AI が Web を操作するための「標準インターフェース（MCP サーバー）」。
*   **DH SaaS Runner**: クラウドホスト型のフルマネージド実行環境。
*   **DH Edge**: クライアントサイド（ブラウザ拡張機能内）で動くプライバシー重視の実行環境。

### 4.2 Dragon Head "Sentinel" (信頼とガバナンス)
リスクを懸念するエンタープライズ向けのセキュリティ・スイート。
*   **Sentinel Governance Hub**: 複数の AI エージェントの行動を一括監視・ポリシー適用。
*   **Sentinel Redactor**: PII（個人情報）を LLM に送る前に Wasm レイヤーで自動マスク。
*   **Sentinel Audit Vault**: SOC2 等の要件に適合した不変ログ保存・SIEM 連携。

### 4.3 Dragon Head "Pilot" (人間との共生 - HITL)
AI と人間がシームレスに協力するためのインターフェース。
*   **Pilot Dashboard**: AI の現在の視界（SoM）をリアルタイム監視。
*   **Pilot Slack/Teams Bridge**: 重要な判断（決済等）の前にチャットツールへ承認リクエストを送信。
*   **Pilot Session Resume**: AI が失敗したセッションを人間が引き継いで操作完了。

### 4.4 Dragon Head "Domain Packs" (知識のマーケットプレイス)
特定の SaaS や Web サービスに特化した、事前定義済みのプラグイン・スキルセット。
*   **ERP Pack**: SAP/Salesforce 等の複雑な業務システム専用の抽出・操作プラグイン。
*   **Compliance Pack**: 業界固有の規制（GDPR/SOX等）に準拠したポリシー・プリセット。
*   **Commerce Pack**: Amazon/Stripe 等の主要 EC サイト操作に最適化されたスキル群。

---

## 5. プライシング・マネタイズモデル

| プラン | 価格体系 | 主な提供価値 |
| :--- | :--- | :--- |
| **Developer** | 無料 | ローカル実行、OSS版バイナリ。 |
| **Pro** | 従量課金 (Usage-based) | クラウドAPI、Semantic Delta、SoM生成。 |
| **Enterprise** | 年間サブスクリプション | 専用インスタンス、Sentinel/Pilot機能、BYOK、SLA、24/7サポート。 |
| **Marketplace** | レベニューシェア | 認定 Domain Pack の売上の 20-30% を手数料。 |

---

## 6. 売上目標 (Sales Targets)

*   **Year 1: 市場適合性の証明 (Product-Market Fit)**
    *   目標: 1億円 (約 $1M) ARR
    *   指標: 有料顧客数 20社、Pro プランの月間 API コール数 1,000万回突破。
*   **Year 2: スケールアップ (Expansion)**
    *   目標: 5億円 ($5M) ARR
    *   指標: エンタープライズ契約 5件、Sentinel/Pilot 機能の導入開始。
*   **Year 3: プラットフォーム化 (Domination)**
    *   目標: 15億円 ($15M) ARR
    *   指標: Marketplace での Plugin 数 500件突破、デファクトスタンダード化。

---

## 7. 販売・チャネル戦略 (Action Plan)

### 7.1 PLG (Product-Led Growth) 戦略 - 「ボトムアップ」
*   **開発者向けベンチマークツールの公開**: SNS (X, Reddit) 等で「トークン代が 1/10 になる」衝撃を視覚化してバズを生む。
*   **MCP エコシステムへの潜り込み**: Claude Desktop 等のデフォルトのブラウザ MCP としての地位を確立。

### 7.2 エンタープライズ営業戦略 - 「トップダウン」
*   **「Trust & Safety」パッケージの提案**: Sentinel によるガバナンスを武器に CISO へアプローチ。
*   **HITL デモによる「安心感」の販売**: Slack 連携デモを使い、経営層に「AI を制御できる」ことを実感させる。

---

## 8. GTM (Go-to-Market) ロードマップ

1.  **Phase 1 (Q1)**: Core 基盤の整備。ベンチマークツール公開による認知獲得。
2.  **Phase 2 (Q2)**: Pro 版 API のベータリリース。スタートアップ 10社による実証実験。
3.  **Phase 3 (Q3)**: Sentinel/Pilot 機能の完成。エンタープライズ PoC 開始。
4.  **Phase 4 (Q4)**: 正式商用化。Marketplace プレオープン。
