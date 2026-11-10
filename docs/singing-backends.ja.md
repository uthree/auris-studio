<a id="singing-backends"></a>
# 歌唱バックエンド

[English](singing-backends.md)

`auris-singer` は共通の `SingerFrames` タイムラインと、必要に応じて並列する `SingerScore` を `SingingBackend` でレンダーします。セッションは `VoiceModel` ファサードを読み込み、具体的なモデル配置には依存しません。これにより、モデル検出、キャッシュ、話者選択、プレビュー、テイクのレンダー、WAV 出力をバックエンド間で同じに保ちます。

<a id="diffsinger-voicebanks"></a>
## DiffSinger ボイスバンク

ボイスバンクの `dsconfig.yaml` を選びます。バックエンドは OpenUtau の配置フィールド `phonemes`、`acoustic`、`vocoder` を読み、音響 ONNX モデルを実行して、そのメル出力とトラックの F0 曲線を ONNX ボコーダーに渡します。設定済み Voices ディレクトリの子フォルダーに置いたボイスバンクは、ライブラリ棚に自動表示されます。

ボコーダーは、ボイスバンク内の `dsvocoder/vocoder.yaml` と、その `model` フィールドが指定する ONNX ファイルとして同梱するか、`vocoder` が指定するボイスバンク相対フォルダーに置きます。音響モデルとボコーダーのサンプルレート、ホップサイズ、メルビン数は一致させます。必要なら、10進対数と自然対数のメル出力を変換します。

音素辞書は行単位テキストまたは JSON のトークン→ID マップです。予約パディング ID と疎なテーブルを含め、JSON の ID は保持します。日本語 IPA は `ja/` 言語プレフィックスを含むボイスバンクの日本語トークンに対応付けます。`use_lang_id` の場合、`languages` JSON が各トークンの言語 ID を指定します。`speakers` は `hidden_size` 個の値を持つリトルエンディアン float32 `.emb` ファイルを指定し、その順序が Auris の話者選択順になります。

energy、breathiness、voicing、tension の埋め込みを使うボイスバンクは、音響モデルの前に言語エンコーダーと分散予測器を実行します。これらは `dsvariance/dsconfig.yaml` の `linguistic` と `variance`、または別の variance 設定がない場合はメイン設定で指定します。予測器には独自の音素辞書と話者埋め込みがあります。音素長と単語長の言語エンコーダーを、音響モデルのフレームグリッドでサポートします。`dsdict.yaml` がある場合はその母音タイプで単語をグループ化し、なければ日本語の母音別名を使います。`../shared/model.onnx` のような補助パスは設定ファイル基準で解決され、バックグラウンド読み込みでは正規化した対象が選択ボイスバンクの内部に残る必要があります。

Auris は記譜したピッチと音素タイミングを提供します。ピッチは対数周波数空間で無音まで延長し、フレームのダイナミクスが合成波形をスケールします。オプションのキーシフトと速度埋め込みには中立値を渡します。現代的なスカラー `steps`／`depth` と、旧式の1要素 `speedup`／`depth` 入力を受け付けます。depth はボイスバンクの `max_depth` に従います。DiffSinger グラフには、訓練済み拡散グラフの読み込みで拡張最適化がクラッシュしたため、ONNX Runtime の基本最適化を使います。GPU 推論がグラフを拒否した場合、自動アクセラレーションは CPU で再試行します。

DiffSinger の書き出しグラフは内部で乱数を生成し、Auris のレンダーシードを受け付けません。保存した音声テイクはレンダー済みの演奏を保持します。

設定の **Set Up DiffSinger…** ボタンは対応する配置フィールドを開き、音素テーブル、音響モデル、ボコーダー設定の存在を検証して、選択したボイスバンクに `dsconfig.yaml` を書き込みます。生成された音声は **DiffSinger** バッジ付きで棚に表示されます。フォームを保存しても既存の話者、言語、variance、その他の配置フィールドは保持されます。

<a id="real-model-verification"></a>
### 実モデル検証

オプトインテストは、必要なら訓練済み音響モデルと variance 予測器、ボコーダーを実行し、波形長、有限サンプル、ゼロでない RMS／ピークを検査します。

```powershell
$env:AURIS_DIFFSINGER_TEST_CONFIG = 'C:/Voices/OpenCpop/dsconfig.yaml'
$env:AURIS_DIFFSINGER_TEST_WAV = 'C:/renders/diffsinger.wav'
cargo test -p auris-singer --test diffsinger_real -- --ignored --nocapture
```

出力フォルダーはあらかじめ存在している必要があります。任意の WAV 設定は並列の `.frames.json` も書き出し、セッションと CLI でレンダーできます。

```powershell
cargo run -p auris-cli -- sing-frames C:/renders/diffsinger.frames.json --voice C:/Voices/OpenCpop/dsconfig.yaml --acceleration cpu -o C:/renders/diffsinger-cli.wav
```

`AURIS_DIFFSINGER_TEST_ACCELERATION=auto` はデスクトップのデフォルトプロセッサー選択を試し、テストのデフォルトは CPU です。CI は辞書、新旧コントロール、両言語エンコーダー、variance 曲線、話者、言語、ダイナミクス、キャンセル用の小さな算術 ONNX 契約を使います。`uv run crates/auris-singer/tests/fixtures/diffsinger/generate.py` で再生成できます。

実モデルプローブは音高のある `a` 母音3つだけを使うため、歌詞発音品質ではなく合成を検証します。Windows では公開 [OpenCpop 配置](https://huggingface.co/spaces/SJTU/diffsinger-webui/tree/main/models/OpenCpop)が、訓練済み言語、variance、音響グラフと NSF-HiFiGAN ボコーダーを通じて、44,100 Hzで87,040個の有限サンプル（1.974秒）を生成しました。自動アクセラレーション実行では、3音の自己相関推定は261.65、329.78、392.11 Hzで、記譜ピッチから0.9セント以内、RMS は0.01249でした。音響設定は変更せず使いました。配置規約は[DiffSinger acoustic exporter](https://github.com/openvpi/DiffSinger/blob/main/deployment/exporters/acoustic_exporter.py)、[variance exporter](https://github.com/openvpi/DiffSinger/blob/main/deployment/exporters/variance_exporter.py)、[OpenUtau renderer](https://github.com/stakira/OpenUtau/blob/master/OpenUtau.Core/DiffSinger/DiffSingerRenderer.cs)に従います。

<a id="leapsinger-voices"></a>
## LeapSinger ボイス

`NAME.leapsinger.json` エントリを選びます。Auris は書き出された LeapSinger 音響モデルと NHVSing ボコーダーを ONNX Runtime から直接実行し、レンダーに Python や稼働中のサーバーは必要ありません。エントリを含む音声フォルダーは、設定済み Voices ライブラリに **LeapSinger** バッジ付きで表示されます。

<a id="prepare-the-models"></a>
### モデルを準備する

[LeapSinger model release](https://github.com/wavtechyukky/LeapSinger/releases/tag/v0.1.0)からチェックポイントを取得し、上流リポジトリで書き出します。以下は上流コミット `6c99e2b4194def02888ada33c20198a16bca2eb5` と、同梱の NHVSing V3.2.1 モデルで検証済みです。3つのチェックポイント話者をすべて公開します。

```sh
git clone https://github.com/wavtechyukky/LeapSinger
cd LeapSinger
git checkout 6c99e2b4194def02888ada33c20198a16bca2eb5
uv venv --python 3.11
uv pip install -e ".[export]" --torch-backend=auto
uv run python -m export.cli \
  --ckpt models/3singer_ritsu3style_uv_gan2d.pth \
  --out auris-voice --model-name acoustic \
  --variant full --speaker embed --num-steps 1
uv run python /path/to/auris-studio/tools/leapsinger/prepare.py \
  --upstream . --checkpoint models/3singer_ritsu3style_uv_gan2d.pth \
  --output auris-voice --name "LeapSinger singers" \
  --speaker-names "御丹宮くるみ" "夏目悠李" "波音リツ"
```

上記の `models/` はダウンロードしたチェックポイントの展開先です。これで `auris-voice/acoustic.onnx` が生成されます。パッケージヘルパーは `checkpoints/nhv_v3_2_1.onnx`、音響リリースのクレジットと規約、チェックポイント固有の音素辞書を `auris-voice/` にコピーし、投影した話者ベクトルを持つ `singer.leapsinger.json` を書きます。最新の上流 exporter はデフォルトで Native DFT を有効にするため、移植可能な ONNX Runtime 推論のために有効のままにします。辞書は1行1音素で、空行と `#` の後のテキストを無視し、最初の音素は `pau` でなければなりません。順序がモデルのトークン ID を決めるため、チェックポイントの `config["phonemes"]` の語彙またはそのモデルに同梱された辞書を使います。新しいリポジトリ辞書はトークン数や順序が違う場合があります。

単一話者を焼き込むには `--speaker bake --spk-id 0 --spk-name singer` で書き出し、ヘルパーに `--acoustic acoustic.singer.onnx --baked` を渡します。最小の baked エントリは次のようになります。

```json
{
  "format_version": 1,
  "name": "LeapSinger singer",
  "acoustic": "acoustic.singer.onnx",
  "vocoder": "nhv_v3_2_1.onnx",
  "phonemes": "ja.phonemes",
  "variant": "full",
  "sample_rate": 44100,
  "hop_size": 256,
  "num_mel_bins": 128
}
```

パスはこのエントリからの相対です。`variant`、`sample_rate`、`hop_size`、`num_mel_bins` のデフォルトは表示値です。**Settings → General → Voice Setup → Set Up LeapSinger…** で生成したエントリを開くか、フォルダーを選びます。名前と acoustic、vocoder、辞書のパスを編集し、full または DiffSinger レイアウトと対応する hop を選びます。full レイアウトの hop は256に固定です。保存時も既存の話者名と投影埋め込みを保持します。**Test Synthesis** はワーカー上で両モデルを読み、CPUで短い母音を検査します。**Register Voice** は ONNX の入出力契約を検証し、エントリを原子的に保存して音声ライブラリへフォルダーを追加し、キャッシュしたメタデータを更新します。競合する編集ではエントリを開き直します。自動レンダーを移植可能にするため、ファイルは音声フォルダー内に解決できなければなりません。棚はマニフェストの音声名を使うため、複数バンクが同じエントリファイル名を持てます。同じエントリは **Track → Choose Voice…**、CLI、モデルツールから選べます。

コードは MIT ライセンスですが、配布される音響チェックポイントと NHVSing 重みには独自の規約があります。リリースのクレジットと付属規約を音声と一緒に保持し、出力と再配布の制限に従います。[LeapSinger license notice](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/LICENSE) と [NHVSing weight terms](https://github.com/wavtechyukky/NHVSing#weight-licensing) を参照してください。

<a id="export-variants-and-speakers"></a>
### 書き出しバリアントと話者

`full` バリアントはネイティブ hop-256 グリッドを使い、明示的な有声／無声曲線を持ちます。UVなしチェックポイント `3speaker_gan2d.pth` では `--variant diffsinger --hop 256` で書き出し、エントリに `"variant": "diffsinger"` を設定します。hop-512 は `--hop 512`、`"hop_size": 512`、`nhv_v3_2_1x.onnx` を使います。パッケージヘルパーにも `--variant diffsinger --hop 512` を渡します。音響モデルとボコーダーは同じグリッドを使う必要があります。

焼き込み話者、または `--speaker none` による単一話者の書き出しは、`name` で指定した1話者として表示されます。`--speaker embed` で書き出したグラフから複数話者を公開するには `speakers` 配列を追加します。各オブジェクトには `name` と浮動小数点値の `embedding` 配列があります。`infer.load_acoustic` でチェックポイントを読み込んだ後、上流の `export.spk_embed.speaker_vector(model, speaker_id).tolist()` で各配列を生成します。これはチェックポイントの生の話者バンク行ではなく、サイズ `model.hidden` の投影ベクトルです。グラフは全エントリに完全なベクトルを要求し、配列順が Auris の話者 ID を決めます。[上流話者ヘルパー](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/spk_embed.py)を参照してください。

<a id="real-model-verification-1"></a>
### 実モデル検証

オプトインテストは各話者について音高のある `a` 母音3つをレンダーし、正確なサンプル数、有限で可聴な音声、周期性、30セント以内の音高を確認します。任意の出力は各話者の WAV と共有 `.frames.json` タイムラインを書き出します。

```powershell
$env:AURIS_LEAPSINGER_TEST_MODEL = 'C:/Voices/LeapSinger/singer.leapsinger.json'
$env:AURIS_LEAPSINGER_TEST_WAV = 'C:/renders/leapsinger.wav'
cargo test -p auris-singer --test leapsinger_real -- --ignored --nocapture
```

出力フォルダーは存在していなければなりません。`AURIS_LEAPSINGER_TEST_ACCELERATION=auto` はデスクトップのデフォルト選択を試し、テストのデフォルトは CPU です。フレームはセッションと CLI にも渡せます。

```powershell
cargo run -p auris-cli -- sing-frames C:/renders/leapsinger.frames.json --voice C:/Voices/LeapSinger/singer.leapsinger.json --acceleration cpu -o C:/renders/leapsinger-cli.wav
```

Windows では v0.1.0 の full/embed モデルと NHVSing V3.2.1 が、CPUと自動アクセラレーションの両方で話者ごとに62,208サンプルを44,100 Hzで生成しました。自動選択もこのマシンでは CPU で完了しました。測定ピッチは261.63、329.63、392.00 Hzから1セント以内でした。UVなし DiffSinger/baked、hop 512、V3.2.1x も合格し、62,976サンプルを生成しました。これらの母音プローブは推論パイプラインと話者入力を検証しますが、歌詞発音品質は測りません。通常の CI は `auris-singer/tests/fixtures/leapsinger` の小さな算術 ONNX グラフでテンソル契約を検査します。

<a id="rendering-and-saved-takes"></a>
### レンダーと保存テイク

LeapSinger は Auris の手動 IPA 修正と音素タイミングピンを消費します。バックエンドは日本語 IPA をモデルのトークンに対応付け、ピッチの隙間を対数周波数空間で補間し、音素クラスから有声フラグを導き、トラックのエネルギー曲線をボコーダー出力に適用します。長いスコアは上限付きチャンクでレンダーし、元のフレームタイムラインに配置します。

上流の音響／ボコーダーグラフは内部でノイズを生成し、レンダーシードを受け付けません。同じスコアを再レンダーすると波形が変わる可能性があります。レンダーしたテイクはプロジェクト内の音声ファイルとして残り、保存時の演奏を保持します。テンソル配置とノイズ動作は上流の [export wrappers](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/wrappers.py) と [excitation graph](https://github.com/wavtechyukky/LeapSinger/blob/6c99e2b4194def02888ada33c20198a16bca2eb5/export/excitation_onnx.py) に従います。

<a id="adding-a-curve-predictor"></a>
## 曲線予測器を追加する

`auris_singer::CurveGenerator` はピッチ、エネルギー、または両方を予測するバックエンドの共通インターフェースです。`SOURCES: CurveSources` はそれぞれを `Host` または `Backend` として独立に宣言します。これらのソースを `VoiceCapabilities::curves` で公開し、形式のデフォルトは `BackendKind::capabilities` に置きます。読み込んだモデル、オプションの予測器を持つネイティブ Auris モデルを含め、`SingingBackend::capabilities` で上書きできます。予測器はインスタンスごとに `CurveGenerator::curve_sources` を上書きすることもできるため、フレームサンプリングと予測が一致するよう、読み込んだモデルの capabilities に同じ値を報告します。

`generate_curves` は、ユーザー編集を適用する前の基本演奏を含む `CurvePrediction<Context>` を返すよう実装します。各予測配列は入力フレームクロック上の宣言済み先頭／末尾コンテキストフレームを含みます。ホスト所有配列は `None` のままにします。`Context` にはデコーダーが必要とするもの（音素タイミング、HTTP クエリ、モデルテンソルなど）を保持し、共有コードは解釈しません。

合成中はスコア、並列フレーム、話者、シードで `prepare_curves` を呼びます。長さと数値を検証し、相対ピッチ編集とエネルギー倍率を予測に適用し、ホスト所有の音響曲線は変更せずコピーします。返された `PreparedCurves` とコンテキストをデコードし、波形から宣言済みコンテキストを切り落とします。進行とキャンセルはバックエンド既存の推論境界で保ちます。

セッションは読み込んだモデルの capabilities を使って `render_frames_with_sources` でフレームをサンプリングします。バックエンド所有のピッチでは自動グライドを省き、バックエンド所有のエネルギーでは一般的な音素レベルとアタック／リリースを省きます。これでアーティキュレーションの二重適用を避けつつ、ベンド、装飾、ベロシティ、表現を保持します。冷たいメタデータ読み取りでは形式のデフォルトを使い、完全なレンダーでは入力をサンプリングする前にモデルを読み込みます。

<a id="voicevox-engine"></a>
## VOICEVOX Engine

[VOICEVOX Engine](https://github.com/VOICEVOX/voicevox_engine) を起動し、設定済み Voices ディレクトリ（または音声ピッカー）に `NAME.voicevox.json` を置きます。これはボイスバンクではなく接続メタデータです。

```json
{
  "format_version": 1,
  "name": "VOICEVOX singer",
  "url": "http://127.0.0.1:50021",
  "sample_rate": 24000,
  "frame_rate": 93.75,
  "styles": [
    {
      "name": "Singer / normal",
      "query_style_id": 6000,
      "decode_style_id": 3001
    }
  ]
}
```

`url`、`sample_rate`、`frame_rate` のデフォルトは上記の標準ローカル Engine 値です。2つの style ID は `GET /singers` で確認します。`query_style_id` は `sing` または `singing_teacher` style、`decode_style_id` は `frame_decode` style でなければなりません。複数エントリは Auris Studio の話者選択肢になります。

バックエンドは歌詞付きスコアを `POST /sing_frame_audio_query` に送り、Engine のピッチ曲線、無声音フレーム、音素音量バランスを保持してから、クエリを `POST /frame_synthesis` に送ります。レンダー時には Engine が稼働していなければなりません。生の `SingerFrames` ファイルには歌詞がなく、このバックエンドではレンダーできません。完全な歌手トラックと音符プレビューはレンダーできます。

記譜したベンドと装飾は各音符のキーに対する予測ピッチを移動し、ベロシティと表現はデコード音声をスケールします。Engine の予測音量は変更せず送ります。デコーダーはそれをレベルではなくアーティキュレーションとして読み、音量を下げると別の子音を歌います（[VOICEVOX pronunciation checks](voicevox-pronunciation.ja.md)参照）。ゲインはフレーム中心間で補間するため、音符間のベロシティ変更でクリックしません。このバックエンドには一般的なグライドや音符アタック／リリースエンベロープを追加しません。制御は休符まで延長され、ホストの音符境界で先行子音がミュートされません。既存の音声テイクは録音時のままなので、この動作を聞くには歌手トラックを再レンダーします。前の音符内で母音に先行する子音も、入ってくる音符のベロシティ、表現、ピッチ編集を使います。静かな前音符やベンドした前音符は変えません。

単独の長音記号（`ー`）は、出力スコア内でだけ前の母音に展開します。`こ・ー・ひ・ー` は `こ・オ・ひ・イ` として送られます。音符、休符、ピッチ、長さ、保存された歌詞は変わりません。前の母音なしの記号は音符番号付きで報告します。HTTP 失敗にはステータスコードだけでなく、拒否された歌詞など Engine の説明が含まれます。

半角カナ、分解された濁点、周囲の空白も出力歌詞で正規化します（` ｶﾞ ` は `ガ` になります）。複数モーラの歌詞は、音符に既存のフレーム長があるため、Engine 用にその長さ全体へ分割します。`スィ`、`クォ`、`キェ` などの拡張単一モーラは単一のままにし、余分な音節へ分割せず子音を保ちます。

VOICEVOX Engine 0.25.2 は子音を前の休符の半分に切り詰めることがあり、2フレームの隙間では子音に約11 msしか残らない場合があります。Auris は前の音符に接続する内部休符で子音長を予測します。予測子音に短すぎる隙間は接続したままにし、長い休止は子音長を再予測せず復元します。復元した音素タイムラインに対してピッチと音量を再計算します。保存音符、母音オンセット、全長は固定です。この修正を聞くには既存テイクを再レンダーしてください。子音が前の短い音符を完全に埋める場合、Auris はその音符の母音に1フレームを確保します。子音の直前の1フレーム音符はこのグリッドで両方の音を収められず、音符番号付きで報告します。その音符を長くしてください。境界休符には自動でパディングを入れます。ゼロ長イベント、無効な MIDI キー、歌詞付き休符は送信前に報告します。

接続の出力サンプルレートは、Engine の各フレームに対して整数個のサンプルに分割できなければなりません。標準93.75 fps Engine には24000または48000 Hzを使います。44100 Hzはこのグリッドを正確に表せないため、接続の読み込み時に拒否されます。これはプロジェクトの音声出力レートとは独立した、音声接続のレートです。

設定の **Set Up VOICEVOX…** ボタンは、Engine URL と query／frame-decode style ID の接続エディターを開きます。同じ画面からローカル Engine 実行ファイルを選択・起動し、`/version` と `/singers` を確認し、Auris Studio の管理 Voices フォルダーに `*.voicevox.json` エントリを保存できます。保存後は、Engine が提供する歌唱スタイルがそれぞれ独立した声として **VOICEVOX** バッジとスタイルのアイコン付きで棚に並びます。選ぶとそのスタイルがエントリに保存されます。歌手一覧を自動で問い合わせるのは同じマシン上（ループバック）の Engine だけで、別マシンへの接続は保存済みのスタイルを表示します。
