<a id="agent-skills"></a>
# エージェントスキル

[English](agent-skills.md)

<a id="auris-composition-skill"></a>
## Auris の作曲スキル

`auris-composition` は、歌詞のあるオリジナル曲やゲーム／映像向けの音楽を作る際に、コーディングエージェントが Auris Studio を使う場面と手順を示します。編集可能なスコア、歌声合成、ループする楽曲、テーマ、場面転換用の曲、プレビュー、レンダリング、ステム、数値で確認できる納品前の検査を扱います。[歌声ワークフロー](../skills/auris-composition/references/vocals.md)では、歌詞とメロディーの開発、声の選択、CLI／MCPでの合成、曲全体のカバレッジ確認を説明します。スキルはデスクトップアプリとは別に配布され、アプリをインストールまたは更新するものではありません。

音楽を納品するときは、[作曲後のアレンジ工程](../skills/auris-composition/references/arrangement.md)も実施します。アレンジは Auris の CLI／MCP ツールで行い、その結果を確認します。作成済みのメロディーと歌詞は保護されます。スキルのリファレンスが扱うのは Auris の操作方法です。作曲・編曲の技法は [composition techniques](composition-techniques.md) にあり、インストール済みのビルドの `search_documentation` で引けます。デスクトップのエージェントとスキルは、同じ文章を読むことになります。

Codex の全プロジェクトで使う場合は、`skills` CLI でインストールします。

```sh
npx skills add uthree/auris-studio --skill auris-composition -a codex -g -y
```

一つのプロジェクトだけで使う場合は、そのプロジェクトのディレクトリで `-g` を外して実行します。

```sh
npx skills add uthree/auris-studio --skill auris-composition -a codex -y
```

インストール済みのスキルの一覧表示と更新には、次のコマンドを使います。

```sh
npx skills list -g
npx skills update auris-composition -g -y
```

`skills` CLI のバージョンによっては対話形式の確認が表示される場合があります。対象のエージェントやインストール先を選びたいときは `-y` を外してください。スキルの原本は、このリポジトリの `skills/auris-composition/` にあります。インストール後は `$auris-composition` と指定して明示的に呼び出すこともできます。

<a id="auris-runtime-prerequisite"></a>
## Auris 本体の準備

[リリースページ](https://github.com/uthree/auris-studio/releases)から Auris Studio を別途インストールするか、ソースからビルドしてください。スキルが提供するのはエージェント向けの手順だけです。`auris`、`auris-mcp`、SoundFont ライブラリ、デスクトップアプリは含まれません。画面を使わずに CLI を操作する前に、`auris --help` が動くことを確認してください。`auris` が PATH にない場合、macOS では `/Applications/Auris Studio.app/Contents/MacOS/auris` または `~/Applications/Auris Studio.app/Contents/MacOS/auris`、Windows では `%LOCALAPPDATA%\Programs\Auris Studio\auris.exe` を使います。`auris-mcp` も同じディレクトリにあります。MCP を使う場合は、ホストの MCP 設定から `auris-mcp` を起動できることを確認してください。スキルは不足している実行ファイルを自動ではインストールしません。見つからない場合はリリースページから導入してください。

<a id="mcp-composition-group"></a>
## MCP のツールグループ

`auris-mcp` は通常、基本的な読み取り専用の探索・ヘルプツールを公開します。`AURIS_MCP_TOOL_GROUPS` でツールを制限している場合は、自動作曲には `composition`、ノート単位の編集には `manual` を有効にしてください。エージェントがルーティングの変更や納品用音声のノーマライズを行う場合は、`mix` も追加します。

```text
AURIS_MCP_TOOL_GROUPS=composition,manual,mix
```

歌声を合成する曲では、`vocals`も有効にしてください（例：`composition,manual,mix,vocals`）。選択したバックエンドには、インストール済みの歌声または設定済みで稼働中のVOICEVOX Engineが必要です。スキルは音声モデルをインストールしたり、不足しているサービスを自動起動したりしません。

Windows では、別のターミナルだけでなく、MCP サーバーのプロセス設定に環境変数を指定してください。macOS/Linux では、`auris-mcp` を起動する環境で変数をエクスポートします。変更後はサーバーを再起動してください。`all` は全グループを有効にします。作曲と納品だけを扱う場合は、上記のグループに絞れます。無効なグループはツールの結果に表示されます。エージェントは使えないツールを推測せず、設定を直してください。
