# LLM時代のWebブラウザランタイム「Dragon Head」の全貌：AIはWebをどう「見る」べきか

## はじめに

生成AI（LLM/VLM）がWeb上のタスクを遂行する「AIエージェント」の開発が急速に進んでいます。しかし、既存のAIエージェントの多くは、人間用に設計された従来のヘッドレスブラウザ（Selenium, Puppeteer, Playwrightなど）をそのまま利用しており、以下のような課題を抱えています。

1.  **トークン効率の悪さ**: 生のHTMLは冗長であり、LLMのコンテキストウィンドウを圧迫し、コストとレイテンシを増大させる。
2.  **ハルシネーション（誤操作）**: AIが認識する座標や要素IDが、レンダリングの微妙な変化でずれ、誤ったボタンを押してしまう。
3.  **待機時間の制御**: 「ロード完了」の定義が曖昧で、固定waitを入れると遅くなり、入れないと失敗する。

これらの課題を解決するために開発されたのが、**AI-Native Headless Browser Runtime「Dragon Head」**です。本記事では、そのコンセプト、アーキテクチャ、そして技術的な差別化ポイントについて解説します。

## コンセプト：Webを「意味的状態（Semantic State）」へ変換するミドルウェア

Dragon Headは、単なるブラウザ操作ライブラリではなく、**WebページをAIが解釈可能な「意味的状態（Semantic State）」へリアルタイムに変換するミドルウェア**です。

人間が目で見て直感的に理解するように、ブラウザがレンダリングした結果（DOM + Style）から「意味のある要素（ボタン、フォーム、テキスト）」だけを抽出し、構造化データ（JSON）としてAIに提供します。これにより、AIはノイズのないクリアな視界でWebを操作できるようになります。

## アーキテクチャ

システムは3層構造と、Rust製のCore Runtimeによる高速なパイプラインで構成されています。

### Layer 1: Core Runtime (Rust)
Chromium CDP (Chrome DevTools Protocol) をラップした基盤レイヤー。レンダリング制御、SRE変換、セキュリティ制御を行います。Rustによるメモリ安全性と高速性が特徴です。

### Layer 2: Plugin Framework (WebAssembly)
特定のサイトや業務に特化した処理を拡張するためのサンドボックス環境。WebAssemblyを採用することで、セキュリティを担保しつつ拡張性を確保しています。

### Layer 3: Skills Engine
一般的なタスク（例：「商品検索」「ログイン」）を宣言的なワークフローとして定義・実行するエンジンです。

## 技術的差別化ポイント

### 1. Semantic Rendering Engine (SRE) によるトークン削減
独自の **SRE (Semantic Rendering Engine)** は、DOMツリーを走査し、視覚的・意味的に重要でない要素（広告、装飾用div、非表示要素など）を徹底的に排除します。
さらに、前回の状態との差分（**Semantic Delta**）のみを生成する機能（開発中）により、LLMへの入力トークン量を平均 **90%削減** することを目標としています。

### 2. 「Stable Key」による堅牢な要素特定
従来のセレクタ（XPathやCSS Selector）や、単純な連番IDは、DOMの軽微な変更や再レンダリングで容易に壊れてしまいます。
Dragon Headは、要素の役割（Role）、正規化されたラベル、DOM構造の署名などから **SHA-256ハッシュによる不変のID (`stable_key`)** を生成します。
万が一、DOM構造が変わって `target_id` (連番) での探索が失敗しても、この `stable_key` を使って要素を再発見（Self-Healing）するロジックを実装しています。

### 3. Time-to-First-Token (TTFT) < 50ms
AIエージェントにおいて、ユーザーが指示を出してからAIが動き出すまでの時間（TTFT）はUXに直結します。
Dragon Headは、画像・動画・トラッキングスクリプトなどの「AIタスクに不要なリソース」をネットワーク層でブロックする **Load Profile** 機能（`minimal` プロファイルなど）を持ちます。これにより、ページのロードと解析を爆速化し、50ms以内での状態提供を目指しています。

## 実装状況 (Current Status)

現在は **Ready for Engineering (v2.1)** フェーズにあり、コア機能の実装が進んでいます。

- **実装済み**:
    - Project Initialization & Rust Workspace setup
    - CDP Client Wrapper (Connection/Page Session)
    - **SRE-01**: Deterministic State Generation (Load Profile: minimal/visual/interactive)
    - **ACT-01**: Stable Key Generation (SHA-256, Collision Handling)
    - **ACT-04**: Robust Action Execution (ID失敗時のStable Keyフォールバック、Verify要求)
    - CI/CDパイプライン (Unit/Integration/E2E with Headless Chrome)

- **開発中/計画中**:
    - **ACT-03**: Semantic Wait (イベント駆動の待機処理)
    - **SRE-02**: Semantic Delta (JSON Patch)
    - **SEC-01/02**: Policy Engine & Session Vault
    - **PLUG-01**: Wasm Plugin System

## 技術的課題と今後の展望

### アンチボット対策
ヘッドレスブラウザ検知（Cloudflare Turnstileなど）への対抗は、イタチごっこの領域です。Dragon Headでは、人間らしい操作揺らぎの導入や、CDPレベルでの指紋対策をPluginとして切り出し、コミュニティベースでアップデートできる仕組みを構想しています。

### 複雑なSPA/Shadow DOMへの対応
ReactやVueで構築された複雑なSPAや、Shadow DOMを多用するサイトでは、SREの解析ロジックが複雑化します。現在、`interactive` プロファイルでのJS実行許可や、Shadow Rootの透過的なトラバーサル処理の最適化を進めています。

### Model Context Protocol (MCP) 対応
AIエージェントとツールの接続標準である **MCP (Model Context Protocol)** に準拠したサーバー機能を実装予定です。これにより、Claude Desktopやその他のMCP対応クライアントから、Dragon Headを即座に利用可能になります。

## まとめ

Dragon Headは、AIのために再発明されたブラウザランタイムです。「人間が見るWeb」から「AIが理解するWeb」への変換を高速・堅牢に行うことで、信頼性の高いAIエージェント開発を支えるインフラとなることを目指しています。

GitHubリポジトリ（現在はPrivate/Internal想定）での開発は活発に進んでおり、コア機能の安定化とともに、セキュリティや拡張性の実装フェーズに入っています。
