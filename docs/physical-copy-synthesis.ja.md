<a id="physical-instruments-against-real-recordings"></a>
# 物理楽器を実録音と照合する

[English](physical-copy-synthesis.md)

物理ピアノ、ギター、ストリングス、リード、ホイッスル、ハンマードストリングスは、固定した**実録音**のコホートと照合して測定している。各ツールは、録音されたノートを実際のRust製の楽器で演奏し、その結果が録音からどれだけ離れているかを報告する。楽器を演奏するのはワーカー `physical_fit_render` である。音に関わる変更をする前にそのコピーを凍結し、変更後に両方のビルドを測定する。これらのツールが楽器を変更することはない。ダウンロード、Python、プロット、試聴ファイルは開発用ツールであり、録音が製品に入ることはない。

現在の楽器の測定値は、楽器の説明とともに[物理楽器](physical-instruments.ja.md)に記載している。このページでは、ツールが何を録音データとし、何を測り、何を必要とするかを説明する。

<a id="piano-guitar-and-violin"></a>
## ピアノ、ギター、バイオリン

[University of Iowa Musical Instrument Samples](https://theremin.music.uiowa.edu/MIS.html) は、Lawrence FrittsとElectronic Music Studiosが作成した。大学は、これらのデータを制限なくダウンロードしてプロジェクトで使うことを認めている。録音の詳細と元のダウンロードリンクは、[ピアノ](https://theremin.music.uiowa.edu/MISpiano.html)、[ギター](https://theremin.music.uiowa.edu/MISguitar.html)、[2012年のバイオリン](https://theremin.music.uiowa.edu/MISviolin2012.html)の各ページにある。

コホートには、Evan Mazunikが演奏したSteinway model B、Brian Penkrotが演奏したRaimundo 118クラシックギター、Leonid Iogansenが演奏したNicolai Tambovskyバイオリンを使う。ギターとバイオリンは無響室で、ピアノは通常の部屋で録音された。そのため、これらの録音との距離にはマイクの特性が含まれ、ピアノでは部屋の響きも含まれる。

元録音は36件、抽出したノートは165個で、pp/mf/ffを速度0.35/0.65/0.95に対応させる。これらは順序尺度の演奏条件であり、測定したMIDI速度ではない。ダウンロードのロックファイルは `tools/eval/references/iowa-captures.json` である。ノートのロックファイル `iowa-notes.json` には、各ソース、切り出しオフセット、音高、測定チューニング、分割、SHA-256を記録する。ダウンローダーは元データのハッシュを検証する。準備ツールではノートロック全体を検証できる。準備したfloat WAVからタイムスタンプ付きPEAKメタデータを除くことで、繰り返し抽出してもバイトハッシュを維持できる。

| 楽器 | 音高の範囲 | 学習ノート | 検証ノート |
| --- | --- | ---: | ---: |
| ピアノ | 48, 52, 55, 60, 64, 67 | 6 | 12 |
| ギター | 40–47, 50–59, 64–71 | 34 | 44 |
| バイオリン | 55–59, 62–71, 76–83 | 30 | 39 |

ピアノの音高52/60/67はホールドアウトする。ギターとバイオリンでは、MIDI番号を3で割った余りが1になる音高をホールドアウトする。学習対象の音高であっても、pp録音はすべてホールドアウトする。この分割は固定したコホートの一部である。楽器の制御値を探索する場合や、`physical-calibration` を有効にしてビルドしたワーカーがリクエストの `dials` で受け取る整音ダイヤルを探索する場合に、読んでよいのは学習ノートだけで、引用する値には検証ノートを使う。検証ノートは別の楽器ではなく、同じ録音セッションに含まれる新しい音高と弱いダイナミクスである。

音高を条件にした倍音追跡で安定した音階音を特定し、上昇順であることを要求する。オンセット検出は最大200 msさかのぼり、局所的な2% RMSしきい値と5 msのガードを使う。可聴倍音から固定チューニングオフセットを推定し、ラベルから75セント以内であることを確認する。弱い倍音は除外する。レンダラーにはノートオン前にこの測定オフセットを渡す。音高、時間アラインメント、ノートごとの合成パラメーターを録音に合わせて調整することはない。

<a id="temporal-mel-distance"></a>
### 時間メル距離

各ノートは24 kHzのモノラルで、キーを押したままオンセット後の最初の**1秒**を収録する。この共通の窓で全録音を扱える。各ノート全体をRMS 0.1に正規化する。個々のフレームは正規化しないため、アタックと減衰を測定でき、時間ワープもない。

2種類のHann STFTで512/2048サンプルの窓と240サンプルのホップを使う。30 Hzから10 kHzを64本の三角形HTKメル帯域で覆い、面積を1に正規化する。パワーは `log(1 + 10000 * mel_power)` で圧縮する。各解像度で、最初の15フレームと残りのフレームについて絶対誤差を別々に平均し、その平均に同じ重みを与える。最終的な距離は、両方の解像度と全ノートを等しい重みで平均する。この定義では小さいほど近い。これは無次元の音響距離であり、知覚品質スコアでもデシベル単位の測定値でもない。位相は捨てられ、倍音は帯域内でしか分解されないため、試聴と音高・安定性のテストは引き続き必要である。定義は `tools/eval/physical_mel.py` にある。

`evaluate_physical_mel.py` は、分割の両側のすべてのノートについて両方のビルドの距離を報告し、各ビルドの工場出荷時パラメーターとハッシュも記録する。どちらのビルドもその工場出荷時の設定で演奏する。Stringsではこれが3人のセクションで、録音のほうはソロバイオリンである。`--production-rate 48000` を指定すると、両方のビルドを48 kHzで演奏し、同じ24 kHzの測定空間にリサンプリングする。

評価では決定的な試聴ファイルも出力する。ピアノ、ギター、バイオリンをそれぞれ**録音 / 前 / 後**の順に並べ、ノート全体のRMSと共通のピーク余裕をそろえる。MIDI 60/55/67に最も近いmf検証ノートをスコア計算前に選び、最も成績のよいノートを選ぶことはない。プロットでは各楽器の3パネルに同じカラースケールを使う。クリックを防ぐため、試聴用の切り出しには境界に5 msのフェードを入れる。スコア計算には加工していないPCMを使う。

<a id="a-steinway-from-c1-to-c8"></a>
## C1からC8までのSteinway

University of Iowaの[Steinway model Bの録音](https://theremin.music.uiowa.edu/MISpiano.html)（演奏はEvan Mazunik）には、C1からC8までのすべてのCのmfと、C2、C4、C6、C7のpp/ffがある。ppのC6とC7は録音のノイズフロアから数dBしか離れていないため測定せず、固定した16件のうち14件を測定する。URL、サイズ、ハッシュは [piano-register-captures.json](../tools/eval/references/piano-register-captures.json) にある。

`piano_register.py` は、オンセットをそろえたノートを24 kHz、25 msごとに、各ファイル自身のノイズフロアを基準に測定する。

| 測定 | 比べるもの |
| --- | --- |
| decay | 最大12個の部分音それぞれの、100 ms 時点の自分に対する音量。レンダーには録音のフロアを加える |
| envelope | 100 ms に対する広帯域の音量。同じフロアを通す |
| spectrum | 50〜400 ms の部分音の音量。合計に対する比 |
| level | 50〜500 ms の音量。開始時に固定したオフセットを通して同じ強さの C4 と比べる |
| knock | 20/60/120 ms での 0.6 f0 未満の帯域パワー。同じオフセットを通した絶対値で、フロアより上だけ |

いずれもデシベル単位の平均絶対距離である。`level` と `knock` はC4で取った強さごとのオフセットを通すが、それ以外の測定は1つのノートの中での相対値である。これらの測定の背後にあるのは、1台の録音されたピアノ、1つのマイク位置、3つの強弱ラベルだけで、ダンパー、リリース、ペダルの参照はない。これらは録音との類似を表すものであり、好みを表すものではない。耳で確かめること。

<a id="b-flat-clarinet"></a>
## B♭クラリネット

[University of IowaのBbクラリネットのMISページ](https://theremin.music.uiowa.edu/MISBbclarinet.html)によると、楽器はBuffet R13 Bbクラリネット、演奏はGlenn Bowenで、1998年8月3日にWendell Johnson無響室でNeumann KM 84を使い、16ビット、44.1 kHzモノラルで録音された。[MISコレクションの利用条件](https://theremin.music.uiowa.edu/MIS.html)は、これらの録音を制限なくダウンロードして任意のプロジェクトで使うことを認めている。オンラインで準備を実行すると、両方のページを元のキャプチャーの横にキャッシュする。

固定したコホートは、`pp`、`mf`、`ff` の `D3B3`、`C4B4`、`C5B5` の録音を使う。発音する音高は34個（MIDI 50–83）、ノートは102個である。ファイル名は実音の音高を表し、D3、C4、C5の基音はそれぞれ約147、262、523 Hzである。楽器がBb管であることを理由に、これらのラベルを移調してはならない。偶数の音高は3つのダイナミクスすべてが学習（51ノート）、奇数の音高は検証（51ノート）に属する。これは独立した楽器や奏者ではなく、同じ楽器、奏者、録音設定の中で音高をホールドアウトするものである。

時系列の領域は、キャプチャーのピークの2%をしきい値とする5 ms RMSエンベロープで検出し、短いギャップは結合する。各ノートは5 msのプリロールから始まり、検出した領域の250 ms後で終わる。その後3秒に切り詰めるかゼロパディングし、ポリフェーズで24 kHzにリサンプリングする。検出した領域の長さをおおよその保持時間として使い、レンダラーでは2.98秒を上限とする。そのため、保持した窓の全体にわたって持続するノートもある。T90はその窓の中でのエネルギーのタイミングを測るもので、完全な物理的リリースを測るものではない。速度は `pp/mf/ff` を0.35/0.65/0.95に対応させる。これは開発上の選択であり、測定したMIDI速度ではない。測定した録音のデチューンはレンダラーに渡す。ソースと準備済みデータのハッシュ、時刻、音高は `tools/eval/references/clarinet-notes.json` に保持する。マニフェストのチェックサムはCRLFをLFに正規化するため、Gitのプラットフォームごとの行末によって無効になることはない。

`clarinet_copy.py` はワーカーの `clarinet`、つまりReeds楽器を測定する。両方の信号に因果的な30 Hzハイパスをかけ、ノート全体のRMSを0.1に正規化する。時間ワープやフレームごとのゲイン調整は行わない。log-melの設定は上記と同じで、512/2048のHann窓、240サンプルのホップ、30 Hzから10 kHzの64帯域を使う。平均絶対距離には、アタック（0–50 ms）、ボディ（50–250 ms）、テール（250 ms–3 s）に0.3/0.3/0.4の重みを与える。あわせて、20 ms RMSエンベロープのdB距離と、帯域制限したエネルギーの90%が含まれる時間であるT90の絶対誤差も報告する。対応ブートストラップは2,000回の再標本化を行い、各音高のすべてのダイナミクスをまとめて扱う。楽器は24 kHzで演奏する。`--rate 48000` を指定すると48 kHzで演奏し、同じ測定のために24 kHzにリサンプリングする。

<a id="hammered-dulcimer-and-tin-whistle"></a>
## ハンマーダルシマーとティンホイッスル

参照マニフェストは `tools/eval/references/folk-notes.json` である。18ノートを含み、内訳はダルシマーの10音高（学習6、検証4）とホイッスルの8音高（学習4、検証4）である。MIDI音高は分割の間で重複しない。ホイッスルの素材は連続した1本のニ長調の音階なので、ノートは8つの独立したテイクではなく、1つの録音からの定常部分の抜粋である。ダルシマーのクリップは公開プレビュー全体だが、利用できる素材は公開されているHQ MP3プレビューをWAVに変換したもので、元のWAVのダウンロードにはFreesoundへのログインが必要である。

ホイッスルの音源は [Wikimedia CommonsのWhistle.wav](https://commons.wikimedia.org/wiki/File:Whistle.wav)（[直接のWAV](https://upload.wikimedia.org/wikipedia/commons/6/65/Whistle.wav)）で、ライセンスはCC BY-SA 4.0である。作者欄は機械可読ではないため、マニフェストはEscola Superior de Música de Catalunya (ESMUC) をクレジットするが、それを作者本人とは断定しない。ダルシマーのプレビューは [iternetconeのFreesoundパック](https://freesound.org/people/iternetcone/packs/19445/) と、マニフェストに記録した個別のソースページから取得したもので、CC BY 4.0である。準備済み参照マニフェストのチェックサムは、CRLFをLFに正規化した状態で `bcf55f3f56d867ec5f3e88aaf7813b9963df7009e891520ded691f182be19016` である。各生キャプチャーと準備済みWAVにも厳密なバイトチェックサムがある。倍音間隔と自己相関によって、C4/D4/E4の弱い基音を、それより強い上位部分音と区別する。別に録音されたHigh C5/High D5は別の音高として扱う。丸めたオクターブの監査結果は `tools/eval/references/folk-pitch-audit.json` に保持する。

`folk_copy.py` はワーカーの `hammered_dulcimer` と `tin_whistle`、つまりHammered StringsとWhistlesの楽器を測定する。レンダラーの速度は0.65に固定し、録音のダイナミクスから推定しない。測定した音源のデチューンをレンダラーに渡し、利用できる抜粋の長さをおおよそのノート保持時間として使う。ダルシマーには、クラリネットと同じ位相重み付きlog-mel距離（アタック30%、ボディ30%、テール40%）、エンベロープ誤差、T90を使い、ホイッスルには定常抜粋のlog-melとエンベロープ誤差を使う。この2つは異なる測定なので、楽器の間で数値を比較してはならない。ノートは因果的な2次30 Hzハイパスの後でRMS 0.1に正規化し、時間ワープやフレームごとのゲイン調整は行わない。ホイッスルの参照は、安定した各音階領域の最初の150 msと最後の100 msを捨てる。比較には、残った定常抜粋の最大1秒と、ノートオンの200 ms後から始まる同じ長さの合成音を使う。T90は保持した3秒の窓の中で帯域制限したエネルギーの90%が含まれる時間であり、利用できるプレビューを超えた完全な物理的減衰を測るものではない。`--rate 48000` はクラリネットと同じである。

各検証結果の対象はホールドアウトした4音高だけなので、2,000回の対応音高ブートストラップによる変化の対応95%区間は広い。これは小さなコーパスに対する回帰測定であり、知覚上の違いの証拠ではない。

`folk_tuning.py` は独立した音高の検査である。24 kHzと48 kHzで、速度0.35、0.65、0.95のノートを1.2秒保持する。ホイッスルはD5–D7（MIDI 74–98）、ダルシマーはC3–C6（MIDI 48–84）が対象である。最初の350 msを捨て、20 Hzからmin(10 kHz, 0.49 × サンプルレート)の範囲で最も強いピークを、補間した基音のピークとあわせて確認する。

<a id="running-the-measures"></a>
## 測定の実行

Pythonツールは `uv` で実行する。Windowsで `uv.exe` がPATHにない場合はインストール先のパスを使い、ワーカーには `.exe` のパスを使う。ツールはスクリプト環境を使うため、学習用の環境やGPUは不要である。フォークのコホートを準備するには、MP3プレビューのために `ffmpeg` もPATHに必要である。クラリネットとフォークのフェッチャーは、`--offline` を指定するとキャッシュ済みのキャプチャーから準備する。

```sh
cargo build --release -p auris-synth --example physical_fit_render
# Freeze this executable before changing any audio code, as <before-worker>.

uv run tools/eval/fetch_physical_reference.py target/physical-reference
uv run tools/eval/prepare_physical_reference.py target/physical-reference \
  --lock tools/eval/references/iowa-notes.json
uv run tools/eval/evaluate_physical_mel.py target/physical-reference \
  <before-worker> <after-worker> target/physical-eval

uv run tools/eval/piano_register.py target/piano-register prepare
uv run tools/eval/piano_register.py target/piano-register evaluate \
  <before-worker> <after-worker> target/piano-register/evaluation.json

uv run tools/eval/fetch_clarinet_reference.py target/physical-next/clarinet-reference \
  --lock tools/eval/references/clarinet-notes.json
uv run tools/eval/clarinet_copy.py target/physical-next/clarinet-reference <after-worker> \
  target/physical-next/clarinet-validation.json --baseline <before-worker>

uv run tools/eval/fetch_folk_reference.py target/folk-physical/reference \
  --lock tools/eval/references/folk-notes.json
uv run tools/eval/folk_copy.py target/folk-physical/reference <after-worker> \
  target/folk-physical/dulcimer-validation.json --model hammered_dulcimer \
  --baseline <before-worker>
uv run tools/eval/folk_copy.py target/folk-physical/reference <after-worker> \
  target/folk-physical/whistle-validation.json --model tin_whistle \
  --baseline <before-worker>
uv run tools/eval/folk_tuning.py <after-worker> target/folk-physical/folk-tuning.json
```

Iowaコホートを正確に抽出するには、記録された依存関係のバージョン（NumPy 2.5.3、SciPy 1.18.1、SoundFile 0.14.0）を固定する。正規のノートマニフェストのSHA-256は `878f66a719dd47400ad6246d9a23c8ecb09676c26837bbfa569993099b76c4bc` である。マニフェストの識別には、UTF-8でエンコードした、ソート済みでコンパクトなJSONを使い、ファイルの行末には依存しない。レポートには、ワーカーのハッシュ、工場出荷時パラメーター、ノートごとの距離をすべて記録する。再ビルドした実行ファイルのハッシュはツールチェーンとビルドパスに依存するため、アーカイブしたワーカーのハッシュと一致することを期待せず、PCMと指標を比較する。

```sh
uv run --with numpy --with scipy --with soundfile --with pytest pytest \
  tools/eval/test_physical_mel.py tools/eval/test_piano_register.py \
  tools/eval/test_clarinet_copy.py tools/eval/test_fetch_clarinet_reference.py \
  tools/eval/test_fetch_folk_reference.py tools/eval/test_physical_ab.py
uv run --with ruff ruff check tools/eval
cargo test -p auris-synth --example physical_fit_render
```

数値テストでは、無音または非有限の候補を拒否し、ゲインと極性に対する不変性を検証し、音高、倍音、アタック、減衰の変化を検出し、抽出とチューニングを確認し、合成したノートから既知の減衰とオンセットを読み戻し、固定した学習／検証の分離を保護する。

<a id="listening-to-two-builds"></a>
## 2つのビルドを聴き比べる

`physical_phrases` は、固定した編集可能なプロジェクトを3つ保存する。ピアノの和音と旋律、ギターのピッキングとストローク、Legato を有効にして音を重ねたバイオリンのフレーズである。それぞれを、変更前に凍結した `auris` CLIと変更後にビルドしたCLIでレンダリングし、ビルドごとのディレクトリに `piano.wav`、`guitar.wav`、`violin.wav` として置く。`physical_ab.py` は、各ペアを変更前、変更後の順につなげ、RMSをそろえて両方のピークを0.9未満に保ち、ゲインをWAVの横に書き出す。こうすれば、音量の変化を音色の変化と取り違えることがない。

```sh
cargo run --release -p auris-session --example physical_phrases -- target/phrases
mkdir -p target/phrases/before target/phrases/after
<before-auris> render target/phrases/piano/piano.auris -o target/phrases/before/piano.wav
<after-auris> render target/phrases/piano/piano.auris -o target/phrases/after/piano.wav
# The same for guitar and violin, then:
uv run tools/eval/physical_ab.py target/phrases/before target/phrases/after target/phrases/ab.wav
```
