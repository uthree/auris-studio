<a id="reusable-sounds"></a>
# 再利用可能なサウンド

[English](user-sounds.md)

インストゥルメントまたはドラムトラックを選び、インストゥルメントとインサートエフェクトを調整してから、トラックインスペクターで **Save as user sound…** を選びます。新しい名前を入力します。保存したサウンドはライブラリの **User sounds** セクションとライブラリ検索に表示され、再起動後も利用できます。名前は空にできず、一意でなければなりません。既存のサウンドが暗黙に上書きされることはありません。

ライブラリまたは検索結果のユーザーサウンドを右クリックすると、次の操作ができます。

- **Overwrite from selected track…**: 先にインストゥルメントまたはドラムトラックを選びます。現在のインストゥルメント設定とインサートが取り込まれ、確認画面に保存するサウンドと元のトラックが表示されます。確認すると保存名を保持したまま内容を置き換えます。
- **Rename sound…**: 他のユーザーサウンドで使われていない空でない名前を入力します。
- **Duplicate sound…**: サウンドの設定とインサートチェーン全体を新しい名前で保存します。名前欄には未使用のコピー名が提案され、保存前に変更できます。
- **Delete sound…**: ライブラリから削除することを確認します。上書きと削除は Undo できません。

トラックを選択していない状態でも、名前変更、複製、削除を実行できます。これらのライブラリ編集はプロジェクトにすでに適用した設定を変更せず、ファクトリープリセットも利用できます。確認前に別のウィンドウが同じ保存済みサウンドを変更した場合、編集は拒否され、ライブラリが更新されます。

保存済みサウンドを適用するには、対象のインストゥルメントトラックを選択します。コマンドはインストゥルメント設定とインサートチェーン全体（エフェクトの順序、パラメーター、バイパススイッチを含む）を置き換えます。Undo では、適用前のサウンドとそのオートメーションを1ステップで復元できます。ノート、トラックレベル、パン、センド、ルーティングは曲に残ります。既存のインストゥルメントとインサートのオートメーションはサウンド適用時に削除され、サイドチェーンのソースはトラック間で引き継がれません。

SoundFont のサウンドはライブラリファイルとバンク／プログラムを記録します。ホストされた CLAP と VST3 のサウンドもバイナリのパスと不透明なプラグイン状態を記録します。これらのライブラリはインストールされたままにしてください。適用した設定はプロジェクトに埋め込まれるため、ユーザーサウンドのライブラリを移動または削除しても既存のプロジェクトは変わりません。

すべての物理楽器には、基本インストゥルメント行の下に **Presets** の開示項目があります。開くと開始パッチを選べます。名前で直接検索することもできます。各パッチは編集でき、エフェクトと組み合わせて新しいユーザーサウンドとして保存できます。

| 基本インストゥルメント | 開始パッチ |
| --- | --- |
| Strings | String section、Violin、Viola、Cello、Contrabass、Sul ponticello、Sul tasto |
| Piano | Grand piano、Felt piano、Bright piano |
| Guitar | Acoustic guitar、Nylon guitar、Steel guitar、Muted guitar、Piezo guitar |
| Electric guitar | Clean electric guitar、Warm electric guitar、Bright electric guitar |
| Bass | Finger bass、Pick bass、Muted bass、Upright bass |
| Bell | Classic bell、Soft bell、Bright bell、Tubular bells、Handbell、Singing bowl |
| Mallet | Marimba、Vibraphone、Xylophone、Glockenspiel、Kalimba、Music box、Hard mallet、Muted mallet |
| Reed winds | Clarinet、Soft clarinet、Breathy clarinet |
| Hammered strings | Hammered dulcimer、Bright dulcimer、Muted dulcimer |
| Whistles | Tin whistle、Soft whistle、Airy whistle、Recorder |
| Choir | Mixed choir、Soprano choir、Tenor choir、Bass choir |
| Drums | Natural drum kit、Dry drum kit、Resonant drum kit |

これらは既存の物理モデルに対する編集可能なパラメーターのバリエーションです。各ファミリーの最初のパッチは、従来の既定パラメーターを保持します。名前は意図した音色または演奏スタイルを示し、バリエーションは同じ基盤モデルを共有します。スコアは移調されないため、意図した音域にノートを書いてください。既存のプロジェクトは保存済みの設定を保持します。
