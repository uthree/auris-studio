<a id="installing-auris-studio"></a>
# Auris Studio のインストール

[English](installation.md)

[リリースページ](https://github.com/uthree/auris-studio/releases) では、デスクトップ用インストーラーとポータブルアーカイブを配布しています。同じビルドから作成したデスクトップ版には、アプリケーション、`auris` CLI、`auris-mcp` サーバー、更新ヘルパー、日本語辞書が含まれます。Linux 版には CLI と辞書が含まれます。内蔵の物理楽器を使うために、別のサウンドライブラリを用意する必要はありません。

インストーラーで導入したデスクトップアプリは、起動直後と起動中の6時間ごとに更新を確認します。現在のプラットフォーム向けインストーラーを含む新しいリリースが見つかると、Auris Studio はバックグラウンドでダウンロードし、リリースメタデータに記載されたサイズと SHA-256 ダイジェストを照合します。プロンプトが表示されたら **Restart and Update** を選ぶか、通常の終了時に適用できるようダウンロード済みの更新を残します。更新ヘルパーは Auris Studio の終了後に更新を適用します。バックグラウンド更新は **Settings → General → Automatic Updates** で無効にできます。

**Help → Check for Updates** を選ぶと、同じ確認をすぐに開始します。ポータブル版とソースからビルドした版では、置き換えの代わりに **Open Release Page** が表示されます。適切なインストーラーがない場合は、リリースページと以下のプラットフォーム別手順を使ってください。

<a id="windows"></a>
## Windows

`auris-studio-v<version>-windows-x86_64-setup.exe` を実行します。インストーラーは現在のユーザーの `AppData/Local/Programs/Auris Studio` にインストールするため、管理者アカウントは必要ありません。スタートメニューのショートカットを作成し、デスクトップショートカットも作成できます。コマンドラインツールは既定でユーザーの `PATH` に追加されます。インストール後に新しいターミナルを開くと、名前だけで `auris` と `auris-mcp` を実行できます。インストーラーは辞書を実行ファイルの隣に配置し、両方のツールがそこから見つけられるようにします。更新ヘルパーは `auris-studio.exe` の隣にインストールされ、デスクトップアプリの更新機能が使用します。`.auris` のプロジェクトファイルは、ダブルクリックすると Auris Studio で開きます。

新しいセットアップファイルを実行すると、同じインストールを手動でアップグレードできます。アンインストールするには Windows の **Installed apps** を使います。アンインストールで削除されるのは、ファイル、`.auris` の関連付け、インストーラーが追加した `PATH` エントリです。プロジェクトとユーザー設定はインストールファイルではないため、そのまま残ります。ポータブル ZIP はフォルダー全体を保持すればインストールなしで実行できます。

Windows v0.10.0 と v0.11.0 は、v0.11.1 以降へ一度手動で更新する必要があります。リリースページから新しいセットアップファイルをダウンロードして実行してください。これらのバージョンでは、インストーラーを起動する前に更新ヘルパーがクラッシュすることがあるため、**Restart and Update** から自分自身を修復することはできません。新しいインストーラーを実行すると、それ以降の自動更新で使う更新ヘルパーも置き換わります。

インストーラーは現在署名されていません。発行元とダウンロード元を確認したうえで、Windows SmartScreen で **More info → Run anyway** の操作が必要になる場合があります。

<a id="macos"></a>
## macOS

`auris-studio-v<version>-macos-universal.dmg` を開き、`Auris Studio.app` を **Applications** にドラッグします。アプリには対応するコマンドライン実行ファイルと辞書が含まれます。

```sh
"/Applications/Auris Studio.app/Contents/MacOS/auris" help
"/Applications/Auris Studio.app/Contents/MacOS/auris-mcp"
```

DMG はシェルの `PATH` にコマンドを追加しません。ターミナルから短い名前で使うには、ユーザーの `PATH` に含まれるディレクトリにリンクを作成します。エージェントからは上記のパスを直接使うこともできます。手動でアップグレードするには、Applications にあるアプリケーションを新しいバージョンで置き換えます。自動更新には、現在のユーザーが `/Applications` または `~/Applications` のアプリバンドルを置き換えられる権限が必要です。ポータブル tar.gz には、アプリバンドル内の同じ実行ファイルを指す `auris` と `auris-mcp` のルート階層リンクが含まれます。

アプリケーションバンドルには `auris-studio` の隣に更新ヘルパーも含まれます。アプリケーションは現在署名も公証もされていません。macOS が初回起動をブロックする場合があります。現在の手動起動手順はリリースノートを確認してください。macOS 13.4 以降が必要です。

<a id="linux"></a>
## Linux

`auris-v<version>-linux-x86_64.tar.gz` を展開し、`auris` を `Dictionary` ディレクトリの隣に置いたままにします。Linux リリースにはデスクトップアプリケーションと MCP サーバーは含まれません。

<a id="building-and-installing-from-source"></a>
## ソースからのビルドとインストール

Rust 1.95 以降と Git を用意し、[開発ガイド](development.ja.md#building-on-each-platform)にある OS ごとの準備を済ませてください。リポジトリを取得し、デスクトップアプリを起動します。

```sh
git clone https://github.com/uthree/auris-studio.git
cd auris-studio
cargo run --release
```

macOS と Windows では、リポジトリのルートで次を実行すると、デスクトップ、CLI、MCP の実行ファイルを Cargo でインストールできます。

```sh
cargo install --locked --path crates/auris-gpui
cargo install --locked --path crates/auris-cli
cargo install --locked --path crates/auris-mcp
```

Linux を含め、CLI だけが必要なら上記の `auris-cli` のコマンドだけを実行してください。Cargo はリリース用の実行ファイルをビルドし、既定では `~/.cargo/bin`（Windows は `%USERPROFILE%\.cargo\bin`）に配置します。このディレクトリが `PATH` に含まれていれば、`auris-studio`、`auris help`、`auris-mcp` を実行できます。アプリバンドルや OS のショートカットは作成されません。更新するには、使いたいリビジョンのソースを取得し、インストールコマンドを再実行します。

Cargo は日本語の歌詞用辞書をインストールしません。漢字の歌詞を使う場合は、[開発ガイド](development.ja.md#building-on-each-platform)の辞書の導入手順に従ってください。

<a id="building-the-installers"></a>
## インストーラーのビルド

リリースにタグを付けると `.github/workflows/release.yml` がポータブルアーカイブ、Windows セットアップ実行ファイル、macOS DMG をビルドします。`workflow_dispatch` を使えば、公開せずにビルドできます。Windows セットアップは `tools/installer/auris-studio.iss` と Inno Setup 6 の `ISCC.exe` を使用します。`StageDir`、`OutputDir`、`AppVersion`、`IconPath` の定義はワークフローから渡されます。macOS のランナーはアプリバンドルを組み立て、`hdiutil` で DMG を作成します。リリースワークフローは、どちらのインストーラーにもまだ署名していません。
