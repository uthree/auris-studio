# Auris Studio へのコントリビュート

[English](CONTRIBUTING.md)

関心を持っていただきありがとうございます。バグ報告、アイデア、ドキュメントの修正、プルリクエスト、どれも歓迎します。

## 生成 AI の利用は歓迎です

Auris Studio 自体が生成 AI を多用して作られているので、一部または全部を AI ツールで書いた Issue やプルリクエストも歓迎します。隠したり断りを入れたりする必要はありません。使ったツールを書いてもらえると助かりますが、必須ではありません。

大事なのは他のコントリビュートと同じで、変更が説明どおりに動くこと、下のチェックが通ること、そして投稿する内容を自分で読んでいることです。

**ひとつだけお願い：大量の投稿は控えてください。** 一度に数件の Issue やプルリクエストは大歓迎ですが、一日に何百件、何千件と送られてもメンテナ一人では読みきれません。エージェントが一度にたくさんの問題を見つけた場合は、関連するものを一つの Issue やプルリクエストにまとめるか、数件だけ出して反応を待ってから続きを送ってください。

## Issue を報告する

[GitHub](https://github.com/uthree/auris-studio/issues) で Issue を作成してください。バグの場合は次を書いてもらえると助かります。

* Auris Studio のバージョンと OS（Apple silicon の macOS、Windows、または CLI なら Linux）
* 何をしたか、何を期待したか、実際に何が起きたか
* あればプロジェクトファイル、ログ、スクリーンショット

英語でも日本語でも構いません。

## 開発用ビルドの準備

Rust 1.95 以降が必要です。

```sh
git clone https://github.com/uthree/auris-studio.git
cd auris-studio
cargo run                        # デスクトップアプリを起動
cargo run -p auris-cli -- help   # コマンドラインフロントエンド
```

macOS は Command Line Tools だけでビルドでき、完全な Xcode は不要です。Windows では Visual Studio C++ Build Tools と LLVM も必要なので、先に Visual Studio の開発者用 PowerShell で `.\tools\setup-windows.ps1 -InstallLlvm` を実行してください。詳しくは[各プラットフォームでのビルド](docs/development.ja.md#building-on-each-platform)を参照してください。

コードを変更する前に、クレート構成については[開発ガイド](docs/development.ja.md)を、作業上のルール（レイヤー構造、リアルタイム処理の制約、プラットフォーム、プロジェクトフォルダ）については [AGENTS.md](AGENTS.md) を読んでください。コーディングエージェントと作業する場合は `AGENTS.md` を渡してください。まさにそのために書かれた文書です。アーキテクチャ全体の説明は `auris_session::guide` モジュールにあります。

```sh
cargo doc -p auris-session --no-deps --open
```

## プルリクエストの前に実行するチェック

CI は macOS と Windows でこれらを実行するので、手元で先に通しておくと往復が減ります。

```sh
cargo fmt --all
cargo clippy --workspace --all-targets
cargo test --workspace
cargo doc --workspace --no-deps   # CI はドキュメントの警告をエラーにします
```

CI が厳しくチェックする点：

* 公開アイテムにはすべてドキュメントコメントが必要で（`#![warn(missing_docs)]`）、ドキュメント内のリンクは解決できなければなりません。
* DSP のコードは「動く」ことではなく、数値（レベル、周波数、長さ）を検証するテストで守ります。
* デスクトップのウィンドウは `auris_gpui::harness` でディスプレイなしにテストできます。実行は `cargo test -p auris-gpui --bins` です。[ウィンドウをテストする](docs/development.ja.md#testing-the-window)を参照してください。
* 作曲のライターや音響関係の定数に触れる場合は、[評価](docs/evaluation.ja.md)にある計測ツールを変更の前後で実行してください。

## プルリクエストの出し方

1. リポジトリをフォークし、`main` からブランチを作ります。
2. 一つのプルリクエストには一つの変更だけを入れてください。大きな一つより、小さな複数のほうがレビューしやすいです。
3. コメントとドキュメントは英語で書きます。日本語版（`*.ja.md`）がある文書を変更する場合は両方更新してもらえると助かります。難しければその旨を書いてもらえればこちらで対応します。
4. 説明欄には、何をなぜ変えたか、どう確認したかを書いてください。見た目が変わる UI の変更ならスクリーンショットがとても役立ちます。
5. ユーザーに見える変更は [CHANGELOG.md](CHANGELOG.md) の次のリリースの節に一行追加してください。説明欄で触れてもらえればメンテナが追加します。

Auris Studio は `0.x` なので、プロジェクトファイルの形式や API はどのリリースでも変わり得ます。互換性ポリシーは [CHANGELOG.md](CHANGELOG.md) を参照してください。古いビルドが新しいプロジェクトファイルを誤読しうる変更では `Project::FORMAT_VERSION` を上げてください（どんなときに上げるかはその定数のドキュメントコメントにあります）。

## リリースの流れ

リリースはメンテナが行います。

1. ルートの `Cargo.toml`（と `Cargo.lock`）のバージョンを上げ、[CHANGELOG.md](CHANGELOG.md) に `## <version> — <date>` の節を追加するプルリクエストを作ります。リリースワークフローはこの節をリリースノートとして使うので、見出しの形式が重要です。
2. マージされて `main` の CI が通ったら、メンテナが `v<version>` タグを push します。
3. タグを受けてリリースワークフローが Windows インストーラー、macOS ディスクイメージ（Apple silicon）、Linux CLI アーカイブをビルドし、[リリースページ](https://github.com/uthree/auris-studio/releases)に公開します。インストール済みのアプリにはアップデートが通知されます。

コントリビューターがリリースのために何かする必要はありません。マージされた変更は次のリリースに含まれます。

## ライセンス

Auris Studio のライセンスは [Apache-2.0](LICENSE) です。コントリビュートを送ることで、その内容が同じ条件でライセンスされることに同意したものとします。
