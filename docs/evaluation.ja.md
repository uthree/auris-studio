<a id="evaluating-what-the-composer-writes"></a>
# 作曲家が書くものの評価

[English](evaluation.md)

リリースビルドには含まれない開発用の測定です。このワークスペースのすべてのレベルとタイミング定数を、レンダーして測定しながら調整してきたのと同じ理由で、ライターやダイヤルの変更はまず数値、そして必ず耳で判断します。

[エージェント作曲の試行](composition-quality.ja.md)では、パネルの完全なタスクを実行し、失敗と出所を保持し、ブラインドの聴取比較を準備します。

[プリセットアレンジ研究](preset-arrangement.ja.md)では、これらの測定を曲全体のトラック参加と密度に対応付け、2026-10-04 の改訂におけるデフォルトシードの選択、新シードのチェック、プリセットごとのトレードオフを記録します。

[物理楽器のコピー合成ループ](physical-copy-synthesis.ja.md)では、実際のピアノ、ギター、バイオリン録音の固定コホートに対する時間メルスペクトログラム距離を測定します。訓練音だけでフィットし、実際の Rust レンダラーで保留した音高とダイナミクスを評価します。この音響距離は、下記のシンボリック測定と学習済み測定を補完します。[グランドピアノのキャリブレーション](grand-piano-calibration.ja.md)では3秒の Steinway 音をフィットし、1、3、6秒で保留音高／ダイナミクスを24/48 kHzで確認します。[鍵盤全体のピアノ整音](grand-piano-register.ja.md)では、C1〜C8 の部分音ごとの2段階減衰、音域ごとの音量、響板のノックを測り、そのコホートのホールドアウト音で検証します。[低音域の修正](physical-low-register.ja.md)では、弓の力の連続性、低音の周期性、ピアノ部分音の伸びを、ペアの試聴ファイルで測定します。[ピアノのストレッチ調律](piano-stretch-tuning.ja.md)では、各鍵の基音、聞こえる音高、オクターブのうなりを平均律と比べて測ります。[長音／軌跡の拡張](physical-trajectories.ja.md)では、3/6秒の音、実録音バイオリンのグリッサンド、録音演奏ガイド、後半サステイン／遷移／リリースの個別損失を追加します。[物理モデルの実験](physical-extensions.ja.md)では、因果的な放射モード、レジスター／ジェスチャーのアブレーション、ペアの信頼区間、同期したピアノペダル参照を追加します。[ベース／バー／ドラムのキャリブレーション](physical-pack.ja.md)では、391個の実録音抜粋、アタック／ボディ／テールの個別損失、スネア／タムの全レジスターに対する音響役割ガードを追加します。[エレキギターの実験](electric-guitar.ja.md)では、独立したピックアップ DI、オーバーサンプリングしたアンプ、ピッチでグループ化した分割によるクリーン／ペダル録音のペアベンチマークを追加します。[民族楽器の実験](folk-physical-instruments.ja.md)では、ハンマーダルシマーの打撃と安定したティンホイッスルの音階抜粋を追加し、異なる音高の検証と固定ソースハッシュを使います。[コーラスのキャリブレーション](choir-copy-synthesis.ja.md)では、実際の持続母音に対してネイティブの声道アンサンブルをフィットし、訓練音24個と別の歌手からの保留音80個を使います。

<a id="native-instrument-upper-midrange-voicing"></a>
## ネイティブ楽器の上中域ボイシング

2026-10-19 の上中域研究は、[スネアとライドの修正](physical-pack.ja.md#targeted-snare-and-ride-voicing)に続くものです。16種類すべてのネイティブ楽器ファミリー、追加の pulse/saw/FM リードパッチ、無変調コントロール、本番のポップ／ロックギターアンプチェーンを検査します。ワーカーは44.1/48/96 kHz、ベロシティ0.35/0.65/1で実際の楽器実装を使い、2,295のペア条件を実行します。各1.8秒キャプチャでは1.2秒保持します。

ピアノ、アコースティックギター、エレキギター、弓奏弦、クラリネット、pulse/saw リードには、3.2 kHzで Q=0.9 のセクションを適用します。最大カットはそれぞれ3.5/2/3/3/2/3 dBです。これは各声部の振幅エンベロープ前に実行し、アコースティックモデルのフィードバックとモード周波数は変えません。ウェット量は1 kHzの基音で最大から2 kHzでゼロまで線形に下がり、ベンド、グライド、音色変更には3 msの平滑化を使います。FM は変調エンベロープが有効な間、最大3 dBを適用します。無変調サイン波は変わりません。プレビュー声は同じ基音保護で2.6 kHzフォルマントを最大20%下げ、下側2つのフォルマントを保持します。物理コーラスの声道は変えません。

以下の中央値は、基音が1.4 kHz未満で、元の2〜5 kHzパワーが可聴パワーの1%を超えるペアプローブ窓を示します。アタックは0〜150 ms、サステインは150〜1200 msです。値はゲイン正規化なしの絶対帯域パワー差です。

| 音色 | Attack 2–5 kHz | Sustain 2–5 kHz |
| --- | ---: | ---: |
| Piano | -2.15 dB | -2.20 dB |
| Acoustic guitar | -1.21 dB | -1.19 dB |
| Electric guitar DI | -1.84 dB | -1.81 dB |
| Bowed strings | -2.43 dB | -2.47 dB |
| Clarinet | -1.65 dB | -1.65 dB |
| Pulse lead, width 0.125 | -2.21 dB | -2.21 dB |
| Saw lead | -2.25 dB | -2.25 dB |
| Factory FM | -2.11 dB | — |
| Preview voice | -1.36 dB | -1.37 dB |
| Pop amplified guitar | -1.80 dB | -1.81 dB |
| Rock amplified guitar | -1.19 dB | -1.55 dB |

レポートには、スペクトル変化と全体の減衰を区別するため、可聴パワーを一致させた後の差分も残します。Hann ピリオドグラムの正規化では30 Hz未満の周波数を除外するため、細いパルスの DC オフセットが上位倍音を隠せません。MIDI 96以上でテストしたすべての音符は、アンプチェーンを含め、サンプル単位でベースラインと一致します。ベース、ベル、マレット、ダルシマー、ホイッスル、コーラス、完全なドラムキット、旧式のノイズドラムも同一です。サイン／三角波とゼロインデックス FM コントロールも同じです。

`tools/eval/references/instrument-harshness.json` にはコホート、ハッシュ、結果を保存します。シンボリックルーラーは変わりません。`music.py` で固定したソロ pulse-lead メロディーを通したところ、同じ条件で TuneJury の差分は -0.0278、MuQ-MuLan の正コサイン差分は -0.000028でした。これらの粗い診断は、音楽的な好みの改善や知覚される音色の改善を示しません。試聴ファイルは共通ゲインと、別ラベルの AC-RMS一致候補を比較し、リミッターは使いません。変更した DSP を再ビルドする前に元のワーカーを保管します。

```sh
uv run tools/eval/instrument_harshness.py <before-worker> <after-worker> target/instrument-harshness/fix
uv run --with pytest --with numpy --with scipy --with soundfile pytest tools/eval/test_instrument_harshness.py
uv run --with ruff ruff check tools/eval/instrument_harshness.py tools/eval/test_instrument_harshness.py
```

<a id="the-symbolic-ruler"></a>
## シンボリックルーラー

```
cargo run -p auris-compose --example measure
```

プリセットとパートごとに1行を出力し、スウィングをストレートに固定した8シードの平均を表示します。そのため、後からフィールが動かした位置ではなく、作曲家が選んだ *パターン* を読み取れます。

* **sync/bar** — グリッドの拍節階層における Longuet-Higgins–Lee のシンコペーションを小節ごとに測ります。0は四つ打ち、バックビートは約3、典型的な最後の16分音符の先取りは4です。グルーヴ研究（Witek et al. 2014）では、快感と動きたくなる感覚はこの尺度の *中央*、逆U字に位置するため、最大化するスコアではなく目標にするダイヤルです。
* **pc-bits** — 曲の音高クラスエントロピーを、長さとベロシティで重み付けします。音階全体に均等なパートは log₂7 ≈ 2.81、ほぼ0はドローン、上限を超えるとキーを失った旋律です。
* **steps% / mean-int** — メロディーが段階進行する割合と、半音単位の平均音程です。メロディーライターの調整に使ったコーパスの基準は68%の段階進行でした。

表の背後にある関数は `auris_compose::metrics` にあり、公開され単体テストされています。将来のコマンドやテストも、例が出力する同じ数値を読めます。

オプションの [作曲探索](composition-search.ja.md) は、書かれた音符イベント数／小節という別の明示的なシンボリック目標を使います。パラメーター範囲と固定した作曲シードのもとで、目標からの負の絶対距離を最大化します。要求した密度に近づくことは測定可能なアレンジの変化ですが、曲が良く聴こえることは示しません。

<a id="inspecting-the-saved-score"></a>
## 保存済みスコアを検査する

```
uv run tools/eval/structure.py before/ --json before-structure.json
uv run tools/eval/structure.py after/ --baseline before-structure.json --json after-structure.json
```

保存された `.auris` プロジェクトから、実際に歌詞を含む歌唱クリップを含め、通常の音符を読みます。クリップ内の小節リズムの反復／多様性、連続するモノフォニックな跳躍、前景／支援のオンセット衝突と発音中の重なり、和音の声部が交差しない最小移動と、入る／出る声部の数を別々に測定します。コード、スタブ、アルペジオは支援として数えます。声部移動にはコードとスタブだけを数えます。無音、休符、クリップ境界は人工的なメロディー跳躍になりません。観測がない比率は `null` です。

レポートは音符の範囲と有限データも検証し、ソースハッシュを保存して、ファイル名の幹でプロジェクトをペアにします。反復、重なり、複雑さには音楽上の用途がありますが、これらの数値のどれも普遍的な品質スコアではありません。これは演奏変換や音声ではなく、書かれたテキストを読みます。ミュートされたクリップ／トラックは音楽測定から除外し、検証では保存された全音符を対象にします。ループ、ルーティング、ソロ、音声クリップはシミュレートしません。

`tools/eval/fixtures/vocal-phrases.asong` は、共有ヴァースと無声音の語尾を持つ固定歌詞プローブを提供します。両方のレンダーで同じ辞書と音声アセットを使います。歌唱音符の楽器試聴が確認するのはメロディー／リズムであり、学習済み歌手の明瞭さではありません。

<a id="the-learned-ear"></a>
## 学習済みの耳

```sh
uv run tools/eval/music.py --preset all --seeds 3 --workdir target/before --json before.json
uv run tools/eval/music.py --preset all --seeds 3 --workdir target/after --baseline before.json --json after.json
```

`--cli path/to/auris.exe`（macOSでは `auris`）を渡してレンダーを固定するか、`--preset` の代わりに既存の WAV／ディレクトリを渡します。どちらのモデルも開発用ツールで、レンダーと推論はローカルで行います。初回実行では公開重み数GBを `target/composition-eval/models` に取得し、その隣に公式 TuneJury の固定コピーを置きます。Python とパッケージは uv が管理します。`--device cpu` がデフォルトで、`--device cuda` には CUDA 対応 PyTorch が必要です。

[TuneJury](https://github.com/yonghyunk1m/TuneJury) はペアの選好を表すスカラー報酬を予測します。公開された主ヘッドは凍結した CLAP-Music と MERT 埋め込みを、学習順 `[CLAP audio, MERT audio, CLAP text]` で使います。事後のジャンルキャプションでは、評価器は作者推奨の空プロンプト手順を使い、テキスト分岐を512個のゼロにします。報酬は未校正で1〜10の範囲はなく、同じチェックポイントと前処理条件でのみ比較できます。

[MuQ-MuLan](https://github.com/tencent-ailab/MuQ) は、`tools/eval/music_prompts.json` のバージョン管理された正とコントラストの説明との一致を評価します。モデルは float32 モノラル **24 kHz** PCM を受け取り、音楽／テキストの共同埋め込みを返します。`positive_cosine`、`contrast_cosine`、`contrast_margin` を計算する前に埋め込みを明示的に正規化します。類似度はプリセットのアイデンティティを表し、好みではありません。候補を比較する前にプロンプトを固定します。

両モデルは同じ決定論的な10秒窓を読みます。デフォルトは先頭・中央・末尾で、`--segments 1` なら中央の1窓です。チャンネル平均を24 kHzへのポリフェーズリサンプリングの前に行い、短いクリップは全コピーを繰り返して残りをゼロパディングします。ゲイン正規化は行いません。TuneJury の内部 CLAP エンコーダーは48 kHzへリサンプルして量子化するため、int16 解像度未満の窓は無音や有限でない入力とともに拒否されます。部分的または無効な測定は診断を保存して失敗終了します。

レポートには窓ごとと平均のスコア、WAV ハッシュ、モデル／ソース改訂、ダウンロードしたアーティファクトのハッシュ、パッケージバージョン、プロンプトを含めます。ベースライン比較は測定条件の変更を拒否します。プリセットごとのペア差分と抜粋位置を平均とともに保持します。テンポが変わると中央の抜粋が移動するため、保持した完全なレンダーも確認します。同一ファイル比較は推論の再現性を確認します。モデル重みは CC-BY-NC 4.0 で、ここでの用途はオフライン研究です。

<a id="controlled-melody-listening"></a>
## 制御したメロディー聴取

`melody_ab.py` は固定した伴奏に器楽リードを移植します。`melody_continuity_ab.py` はレンダーを固定し、レベルを一致させたコーラス抜粋を作ります。`melody_phrase_ab.py` はベースライン、ピッチ、リズム、統合バリアントを、明示的なアーティファクトハッシュとコンポーネント変更ガード付きで比較します。既存の音符、伴奏、ID、ミキサー、演奏変換は編集可能なプロジェクトに残ります。`structure.py`、`melody_continuity.py`、`seed_metrics.py` はモデルスコアとは独立に、書かれたスコアのリズム、フレーズ連続性、重なり、声部移動を検査します。

**聴取抜粋**を `music.py --segments 1` で採点します。4条件の `melody_phrase_listening.py --scores scores.json` 仕様は schema_version 1 と `variants` オブジェクトを持ちます。各条件は評価レポート用の `music: {path, sha256}` と、完全なラベルから WAV ハッシュへの対応表 `excerpt_sha256` を渡します。レポートはモデル、前処理、プロンプトのメタデータを共有しなければなりません。スコアを省略するとセルは空になります。`seed_listening.py --manifest manifest.json --scores music.json --output listening.html` はシード比較をシャッフルして生成し、採点ハッシュが聴取抜粋と一致することを確認します。両レポートはローカルの音声プレーヤーを使い、何もアップロードしません。

<a id="preset-tuning"></a>
## プリセットの調整

```sh
uv run tools/eval/tune.py --preset all --trials 18 --out tune-results.json --workdir target/tuning
uv run tools/eval/tune.py --preset chiptune --dials humanize dynamics --trials 12 --out focused.json
```

Optuna TPE は TuneJury 報酬と MuQ-MuLan 正コサインという2つの目的を別々に最大化します。連続ダイヤルは humanize、dynamics、fill、variation、mood、テンポ±6%、スウィング±6ポイント（すでにスウィングするプリセットのみ）です。キー、グルーヴ、進行、形式、楽器は固定します。試行0は現在のプリセット、訓練シードは101/102、独立した検証シードは301/302です。`--dials` は指定したパラメーターに実験を限定し、テンポやスウィングを含む他の解決済み値はすべて固定します。

2つの訓練目的が試行0以下にならない Pareto 候補を選び、その中では報酬を優先します。承認には、両方の検証平均が現状以上で、少なくとも一方が改善することが必要です。各試行は実効ダイヤル設定、編集可能なプロジェクト、完全なレンダー、窓位置、PCM ハッシュ、スコアを別のファイル名で保持します。結果はプリセットごとに保存します。訓練だけの改善では `preset.rs` を更新する理由になりません。

[2026年10月のキャリブレーション](learned-music-tuning.ja.md)には、出荷したダイヤル、ライターの変更、新しいペア測定、却下した候補を記録しています。

ライターを変更するときは、元のレンダーをアーカイブし、編集前にシンボリックルーラーと `music.py` を実行します。音楽上のルールを1つ変更し、同じ測定器とレンダー設定で一致するシードを比較します。失敗候補と回帰を保持します。モデルに導かれた選択後の最終的な一般化チェックには新しいシードを使います。検証シードを繰り返し使うと訓練の証拠になるためです。

これらのモデルは測定可能な最適化目標を提供しますが、聴取の代わりにはなりません。平均報酬とともに、音色のアイデンティティ、スコアの多様性、音符の範囲、ケースごとの回帰を確認してください。1つの報酬を最適化すると、より良い音楽的展開ではなく、反復したテクスチャーや特定レンダーのアーティファクトが報われることがあります。

<a id="verify-the-measuring-tools"></a>
## 測定ツールを検証する

```sh
uv run --python 3.11 --with pytest --with numpy --with scipy --with soundfile --with optuna pytest tools/eval/test_music.py tools/eval/test_render_audio.py tools/eval/test_tune.py
uv run --with ruff ruff check tools/eval/music.py tools/eval/learned_models.py tools/eval/render_audio.py tools/eval/tune.py
```

契約テストはモデルのダウンロードを必要としません。実モデル比較では固定した評価環境を使い、保持するすべてのアーティファクトを記録してください。
