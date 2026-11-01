<a id="development"></a>
# 開発

[English](development.md)

ワークスペースの構成、各プラットフォームでのビルド、楽器やエフェクトの追加方法を説明します。

<a id="where-things-are"></a>
## 構成

```
BACKEND — UI 依存なし
  crates/auris-core     型、音楽理論、プラグイン trait、プロジェクトモデル — ローカル依存なし
  crates/auris-dsp      エフェクトと DSP プリミティブ
  crates/auris-synth    組み込み楽器
  crates/auris-sampler  SoundFont 再生: フォントバンクとサンプラー楽器
  crates/auris-clap     外部 CLAP プラグインのホスト — auris-core のみ依存
  crates/auris-vst3     外部 VST3 プラグインのホスト — auris-core のみ依存
  crates/auris-engine   レンダーグラフ、トランスポート、cpal 入出力、オフラインレンダー
  crates/auris-io       音声ファイル入出力、プロジェクト保存／読み込み
  crates/auris-gpu      オフライン分析用のオプション wgpu 計算
  crates/auris-analysis DSP とオプションのローカルモデルによるオフライン音楽認識
  crates/auris-compose  スコアベースの自動作曲: テキスト仕様を受け、音符を出力
  crates/auris-vocal    歌唱: 歌詞から IPA 音素、音符から声モデルフレーム
  crates/auris-singer   歌唱: DiffSinger、LeapSinger、VOICEVOX、オフライン
  crates/auris-i18n     全言語の UI テキスト、それ以外は持たない
  crates/auris-session  ドキュメント、エンジン、フロントエンドが必要とする全コマンド
  crates/auris-toolbox  言語モデル用ツールとしてのコマンド、両モデル接続で共有
  crates/auris-video    ノートロール動画: フレームをソフトウェア描画し、導入済み ffmpeg でエンコード

FRONTEND
  crates/auris-gpui     デスクトップアプリケーション（バイナリ: auris-studio）
  crates/auris-cli      コマンドラインツール（バイナリ: auris）
  crates/auris-mcp      Model Context Protocol サーバー（バイナリ: auris-mcp）
  crates/auris-agent    モデルクライアント: Ollama / OpenAI 互換（ライブラリ: バックグラウンドモデルワーカー）

アプリケーションと並ぶもの
  crates/auris-update-helper  アプリ終了後に導入済みバイナリを置き換える
                              （バイナリ: auris-update-helper）

ワークスペース外
  vendor/rustysynth     他者の crate、fork — 専用 README を参照
  vendor/midir          MIDI ライブラリの fork — README-AURIS.md を参照
  vendor/vst3-host      VST3 ホストの fork — README-AURIS.md を参照
  vendor/gpui-flow      ミキサーグラフ UI の fork — README-AURIS.md を参照
```

依存関係は厳密に下向きで、各 crate が *名前を付けてよいもの* によって境界を強制します。上のツリーはリポジトリの地図です。ワークスペースが実際にどう組み合わさるか、レイヤー規則と各境界の理由、2つのスレッドが仕事を受け渡す方法、リアルタイム契約、プラグインの書き方、作曲形式、2つのプラットフォームの違いは **`auris_session::guide`** に記載されています。

```bash
cargo doc -p auris-session --no-deps --open
```

`auris-session` はすべてのバックエンド crate に依存するため、ガイドから API ドキュメントへ直接リンクできます。アーキテクチャの説明はそこに置き、ここにはビルド手順を置きます。

<a id="building-on-each-platform"></a>
## 各プラットフォームでのビルド

`cargo run` と `cargo run --release` は、ネイティブ物理楽器を再生できる状態でデスクトップを起動します。[physical instruments](physical-instruments.ja.md) にモデルと操作があります。

リリースアーカイブには NAIST 日本語辞書が含まれますが、ソースチェックアウトや新しい Git worktree にはありません。Cargo はコードだけをビルドします。漢字歌詞を試す前に一度辞書を取得します。

```powershell
# Windows / PowerShell
.\tools\fetch-dictionary.ps1
```

```bash
# macOS / Linux / Git Bash
tools/fetch-dictionary.sh
```

両スクリプトは `auris dictionary --manifest` の固定マニフェストを読み、アーカイブを検証し、ライセンスとともに `Dictionary/naist-jdic` にインストールします。取得後にデスクトップを再起動すると自動的に辞書を読み込みます。`cargo run -p auris-cli -- dictionary` で検出を確認できます。PowerShell では `-AurisPath .\target\debug\auris.exe` も使えます。

worktree 間で共有するコピーには、PowerShell で `-Destination "$HOME/.config/auris-studio/Dictionary"` を渡すか、Bash スクリプトに `"$HOME/.config/auris-studio/Dictionary"` を渡します。`AURIS_CONFIG_DIR` が設定されていれば、その `Dictionary` サブディレクトリを使います。明示的な `AURIS_DICTIONARY` は `naist-jdic` の親を指し、自動検索を置き換えます。

<a id="windows"></a>
### Windows

Rust MSVC ツールチェーンと Visual Studio C++ Build Tools を使い、開発者 PowerShell から実行します。

```powershell
cargo run
```

デフォルトの音声バックエンドは WASAPI です。ASIO には `asio` feature と LLVM/Clang、ASIO SDK が必要です。

```powershell
.\tools\setup-windows.ps1 -InstallLlvm
cargo run --features asio
```

4つのフロントエンドすべてでこの feature を使えます。`cargo run -p auris-cli --features asio` もその一例です。Windows リリースは4つすべてを `--release --features asio` でビルドします。ローカルの同等ビルドには `cargo build --release --locked --features asio` を使います。`--release` だけでは ASIO は有効になりません。

セットアップスクリプトは既存の LLVM を使うか、Chocolatey／WinGet でインストールし、現在の PowerShell に `LIBCLANG_PATH` を設定します。新しいターミナルでは再実行してください。独自のインストールには `-LlvmBin 'D:\LLVM\bin'` を使います。CI の ASIO チェックと Windows リリースビルドも同じスクリプトを使い、CI はデフォルト構成と ASIO 構成の両方をテストします。`asio-sys` の `Unable to find libclang` は依存関係不足か誤った `LIBCLANG_PATH` を示します。Visual Studio C++ ワークロードだけでは [bindgen](https://rust-lang.github.io/rust-bindgen/requirements.html) が必要とする `libclang.dll` は提供されず、Rust に付属する LLVM もこの依存関係の代わりにはなりません。

`asio` を有効にすると初回ビルドで CPAL が ASIO SDK をダウンロードします。展開済みコピーには `CPAL_ASIO_DIR` を設定します。デフォルトビルドには ASIO SDK も LLVM も不要です。

Settings → Audio でドライバー、デバイス、サンプルレート、要求バッファサイズを選択します。WASAPI は共有モードで、ASIO は同じインターフェースを入出力に使います。ドライバー変更は再生を停止してモニターを開き直し、録音中は設定できません。適用されたバッファ表示は実行中のバックエンドの値で、ドライバーに制限されると要求値と異なります。これは1バッファの時間であり、キーから音までや入出力往復の遅延ではありません。128または256フレームから始め、ドロップアウト時に増やします。特定のインターフェースの遅延はハードウェアループバックで測定します。

macOS の ⌘ コマンドはここでは Ctrl（音符を置く ⌘-click も含む）です。設定ウィンドウには前面のキーボードに対応するキーが表示されます。Windows にはシステムメニューバーがないため、メニューバーはウィンドウ内に描画されます。`auris-gpu` は Windows で Vulkan を使い、プラグインホストは Windows 0.62、リリース GPUI は Windows 0.61 を使います。GPU 分析はオプションで、アダプターがなければ CPU に戻ります。

<a id="macos-without-a-full-xcode-install"></a>
### 完全な Xcode インストールなしの macOS

macOS ビルドには13.4以降が必要で、付属 Core ML ONNX Runtime に合わせています。ワークスペースの Cargo 設定は両アーキテクチャにこの deployment target を設定し、アプリバンドルと Intel 用ランタイムも同じ最小バージョンを使います。gpui は通常 `xcrun metal` でビルド時にシェーダーをコンパイルしますが、Xcode 内にあるため `xcode-select -p` が Command Line Tools を指すと到達できません。このプロジェクトは `gpui/runtime_shaders` を有効にし、起動時に Metal framework 経由でコンパイルします。設定はすでに `Cargo.toml` にあり、Xcode があっても保持してください。起動時のシェーダーコンパイル1回が必要ですが、15 GBのダウンロードなしにビルドできます。

<a id="adding-a-sound-source"></a>
## 音源を追加する

`Parameterized`（パラメーターを宣言し ID で読み書きする trait）と、`Instrument` または `Effect` の2つを実装して factory を登録します。宣言したパラメーターがエディターになり、UI は記述子から適切なウィジェット、範囲、単位、スケールで生成されます。実例は **`auris_session::guide::plugins`** にあり、`prepare` と `process` の境界、パック全体の登録、factory closure が運べない依存の扱いも説明します。

```bash
cargo doc -p auris-session --no-deps --open
```

ガイドの例は doctest としてコンパイルされ、テストスイートが実行します。trait と合わなくなった断片はビルドを失敗させます。

<a id="the-commands"></a>
## コマンド

```bash
cargo test --workspace                    # unit tests
cargo clippy --workspace --all-targets    # lints
cargo fmt --all                           # formatting
cargo doc --workspace --no-deps --open    # the API documentation
```

デスクトップは `auris-agent` ライブラリをリンクし、Agent Panel 用のキャンセル可能なバックグラウンドスレッドを起動します。`cargo run` はパネルに必要なものをすべてビルドします。ワーカーは非同期ランタイムを所有し、チャネルで UI と通信します。権限とドキュメント編集は UI スレッドで行います。会話の停止・置換は、UI をブロックせずに保留中のモデル待ちと承認待ちをキャンセルします。プロバイダーのモデル検出もタイムアウト付きのバックグラウンドスレッドで実行されます。どちらの経路もプロセスを起動せず、プロセスの作業ディレクトリを変更しません。各 crate の `#![warn(missing_docs)]` と警告拒否の CI により、公開項目の doc comment 不足や解決不能リンクはビルド失敗になります。ガイドの例は doctest なので、`auris_session::guide` のシステム説明が実装からずれるとビルドが知らせます。

<a id="testing-the-window"></a>
## ウィンドウをテストする

Window メニューは各ドックパネルをメイン／別ネイティブウィンドウ間で切り替えます。レイアウトは選択、表示、戻り先ドックを保持します。切り離したパネルを閉じると隠れ、View で再び開き、Window のチェックを外すとドックへ戻ります。プラグイン、ミュージカルタイピング、分析結果、音色マップもネイティブウィンドウで、1つの `AurisApp` とセッション、選択、編集、Undo を共有します。

`auxiliary_window.rs` は描画終了までネイティブウィンドウの作成・削除を延期します。GPUI は同期描画するため、ドキュメントエンティティの借用中に開くと再入します。入力ルーティングは全ウィンドウで共有し、暫定メニューとテキストエディターは開いたウィンドウに属します。各パネルを1つのウィンドウだけに描画して、ジェスチャー座標と入力ハンドラーを明確にします。

プロジェクト、Settings、voice setup は `crates/auris-gpui/src/titlebar.rs` のタイトルバーを共有します。ネイティブフレームを透明にし、テーマから内容を供給します。操作コントロールは専用ドラッグ領域の隣に置きます。Windows では親のドラッグヒットボックスが子の入力を奪うためです。ネイティブのウィンドウ操作領域は Snap と最大化／復元を保持し、閉じるボタンはメインウィンドウの未保存ドキュメントガードを呼び出します。ドラッグ領域は祖先のフォーカスハンドラーを覆うため、ネイティブのウィンドウ移動を妨げません。[Zed のタイトルバー](https://github.com/zed-industries/zed/blob/main/crates/title_bar/src/title_bar.rs)と[Logic Pro のコントロールバー](https://support.apple.com/en-euro/guide/logicpro/lgcp5bdd6d9d/mac)をレイアウトの参考にしています。タイトルバーの高さは `AurisApp::chrome_height` に含め、両プラットフォームでドラッグ、サイズ変更、コントロールを確認します。

`crates/auris-gpui/src/harness.rs` はディスプレイ、GPU、オーディオデバイスなしでアプリ全体をウィンドウに開き、`cargo test` から操作します。これを可能にするプラットフォームは gpui が提供し、この crate の dev-dependency である `gpui/test-support` が有効化します。

```bash
cargo test -p auris-gpui --bins        # this crate is a binary, so `--lib` finds nothing
```

ピクセルとハードウェア以外は実物です。実際のキーマップ、ビュー階層、セッション、コマンドを使い、キーを押し、名前でコントロールをクリックし、レーン上でポインターをドラッグして、ドキュメントに何が起きたかを確認します。ヘルパーは `open`、`with_a_clip`、`paint`、`click`、`choose`、`drag`、`lane_point`、`roll_point` です。それぞれ用途とコストを説明する doc comment を持ちます。

確認できないことは2つです。**ピクセルをアサートしてはいけません。** `NoopTextSystem` はすべてのグリフを同じメトリクスで配置し、シーンをラスター化せず破棄します。色、間隔、可読性は人が確認します。**トランスポートは観測できません。** `Session::is_playing` と playhead は audio thread が書く atomic で、デバイスなしのセッションにはそのスレッドがありません。Play と Seek は送られますが返りません。ドキュメントとビュー状態をアサートしてください。このため、ウィンドウが到達できない判断もテスト可能な自由関数に置きます。例は `crate::ui::context_menu::clips::splittable` です。各 `icon_button`、`button`、コンテキストメニュー行、メニューバー行にはハーネスが名前で探せる名前があり、`debug_selector` は gpui を `test-support` 付きでビルドしない限り何もコンパイルしません。`test-support` が有効になるのは `cargo test` のときだけです。

<a id="live-audio-visualizer"></a>
## ライブオーディオビジュアライザー

**Window > Audio Visualizer** でポストフェーダー信号を確認します。デフォルトはマスター、**Follow selected track** は選択トラックに追従します。EQ エディターには独立タップがあり、異なるストリップを両ウィンドウで表示できます。**Waveform**、**Spectrum**、**Stereo**、スクロールする **All** を選びます。メーター単独に効くコントロールはそのメーター内に置かれ、すべてのコントロールはコマンドパレットからも利用できます。

スペクトラムは等パワーのステレオ平均、片チャンネル、各ビンの L/R 最大値、モノラル合計を表示します。**Average** は時間ベースの200 ms応答で電力を平滑化するため、再描画の遅延で意味が変わりません。**Peak hold** は帯域ごとの観測最大値、**Freeze** は現在の表示を保持します。**Save comparison** はトラック変更後も表示中のスペクトラムを保持し、**Clear comparison / peaks** は新しい比較を開始します。波形またはステレオだけの表示では FFT を省略します。色はアクティブテーマのトラックパレット（カスタム色を含む）に従い、波形は2チャンネルを区別し、スペクトラムは低域から高域へパレットを補間します。**Effects** は波形ハロー、スペクトラムのシェーディング、ステレオの軌跡だけを切り替え、測定値は変更しません。

オシロスコープは左右のサンプル振幅を時間に対して直接描き、自動トリガーは音量の大きいチャンネルの立ち上がりゼロクロスに合わせます。これにより再生を遅延・変更せず周期信号を安定させます。audio thread は2048サンプルの循環履歴へ新しいブロックだけを書き、表示は古い半分からトリガーを探して最新1024サンプル以下を描きます。**Time span** は1/4、1/2、全表示窓、**Display gain** は1x、2x、4x、自動を巡回します。どちらも表示だけを変更し、フルスケールを超える値は描画時だけクリップします。

ステレオスコープはサイドを横、ミッドを縦に描き、モノラルは縦線、逆極性は横線になります。自動ゲインは表示だけに作用し、5本のフェード軌跡を使って無制限の履歴なしに動きを見せます。相関は -1〜+1で直近3秒に見えた範囲を示し、最小値は **Reset correlation** まで保持します。片方または両方が無音で現在値が未定義ならダッシュを表示します。負の相関はモノラルで聴く理由であり、音楽品質の自動評価ではありません。ビジュアライザーはラウドネス／トゥルーピークメーターではありません。

CLAP の再生停止、パラメータ表示のスナップショット、再現可能な同時再生プローブについては、[ホストプラグインの再生パフォーマンス](hosted-plugin-performance.ja.md)を参照してください。
