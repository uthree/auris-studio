> [!WARNING]
> **このブランチは非推奨で、マージされません。** `codex/render-pipeline-design`
> （型付きレンダーパイプライン、独自の format 33）は 2026-10-30 に開発を打ち切りました。
> 同じ発想は `main` 上で、形式互換の小さな形（別トラックの音符に追従する「レイヤー」、
> uthree/auris-studio#55）として作り直しています。このブランチを新しい作業の土台にしないでください。
> また、このブランチで保存したファイルを `main` のビルドで開かないでください。ここでの format 33 は
> `main` の format 33 とは別物です。参照用に残しているだけで、最終状態はタグ
> `archive/render-pipeline-design` に記録しています。

<a id="auris-studio"></a>
# Auris Studio

[English](README.md)

Auris Studio は Rust で開発されたデジタル・オーディオ・ワークステーションです。macOS と Windows に対応し、[gpui](https://crates.io/crates/gpui) を使った GPU 描画の UI を備えています。楽曲の編曲・編集、テキストで指定した条件に基づく作曲、録音、CLAP・VST3 プラグインの利用、歌声のレンダリングができます。CLI、MCP、エージェント用の各フロントエンドは、デスクトップ版と同じ編集コマンドを使います。

<a id="install"></a>
## インストール

[リリースページ](https://github.com/uthree/auris-studio/releases)から Windows 用インストーラー、または macOS 用ディスクイメージをダウンロードしてください。デスクトップアプリ、`auris` CLI、`auris-mcp` が一緒に含まれます。Linux 向けには CLI のアーカイブがあります。ポータブル版とコマンドのパスについては[インストールガイド](docs/installation.ja.md)を参照してください。内蔵の物理モデル音源は追加の音源ライブラリなしで使えます。

Rust 1.95 以降を使ってソースからビルドする場合は、次を実行します。

```sh
cargo run --release
```

Windows でのビルドには Visual Studio C++ Build Tools と LLVM も必要です。Visual Studio の開発者用 PowerShell で `.\tools\setup-windows.ps1 -InstallLlvm` を実行してからビルドしてください。各 OS の準備については[開発ガイド](docs/development.ja.md#building-on-each-platform)、内蔵音源については[物理モデル音源](docs/physical-instruments.ja.md)を参照してください。

<a id="install-the-composition-agent-skill"></a>
## 作曲エージェント用スキルのインストール

任意で使える `auris-composition` スキルはコーディングエージェントによる Auris Studio での作曲、レンダリング、成果物の確認を支援します。Codex にインストールするには、次を実行します。

```sh
npx skills add uthree/auris-studio --skill auris-composition -a codex -g -y
```

このコマンドはスキルを全プロジェクトで使えるようにします。現在のプロジェクトだけに入れる場合は、そのプロジェクトのディレクトリで `-g` を外して実行してください。スキルには Auris Studio のアプリや CLI は含まれません。アプリや CLI は上記の手順で別途インストールします。更新方法、実行ファイルのパス、MCP の設定は[エージェント用スキル](docs/agent-skills.ja.md)を参照してください。

<a id="documentation"></a>
## ドキュメント

- [機能と使い方](docs/features.ja.md)
- [自動作曲](docs/composition.ja.md)
- [エージェントと MCP のワークフロー](docs/agent-workflows.ja.md)
- [インストールとポータブル版](docs/installation.ja.md)
- [作曲エージェント用スキルの導入](docs/agent-skills.ja.md)
- [歌声合成バックエンド](docs/singing-backends.ja.md)
- [開発](docs/development.ja.md)と[作曲の評価](docs/evaluation.ja.md)
- [変更履歴と互換性ポリシー](CHANGELOG.md)

ライセンスは [Apache-2.0](LICENSE) です。
