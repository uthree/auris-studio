<a id="agent-composition-quality-trials"></a>
# エージェント作曲品質の試行

[English](composition-quality.md)

`tools/eval/composition_quality.py` は、分離した未保存フィクスチャで実際のエージェントパネルホストを実行します。バージョン管理されたマニフェストには開発用タスク8件とホールドアウトタスク4件があります。ベースラインを集める前に固定し、ホールドアウトの出力に対して調整しないでください。両条件で同じコントローラーモデル、バックエンド、マニフェスト、反復回数を使います。ランナーは同じライブアプリケーションホストを通じて Codex CLI、Claude Code、Rig/Ollama をサポートします（`--backend codex|claude|rig`、`--model <name>`）。

GPUI の **test binary** をビルドし、Cargo の JSON 出力から実行ファイルを見つけて、ランナーに渡します（`auris-studio.exe` ではありません）。ランナーは実行前にバイナリとランタイム DLL をコピーします。出力ディレクトリは新規に用意してください。

```powershell
cargo test -p auris-gpui --bins --no-run --message-format=json > build.jsonl
uv run tools/eval/composition_quality.py run --executable <test-executable> --output target/quality-before --repetitions 3
# Rebuild after the implementation change, then collect the matching condition.
uv run tools/eval/composition_quality.py run --executable <test-executable> --output target/quality-after --repetitions 3
uv run tools/eval/composition_quality.py blind --before target/quality-before --after target/quality-after --output target/quality-listening
```

対象を絞った実験には `--case authored_folk` を使い、両側で同じコホートを保持します。開発上の判断を固定した後だけ `--split holdout` を使います。CLI 試行には対応する認証済み CLI が必要です。Rig 試行には稼働中のローカル Ollama サーバーと、ツールを使えるモデルが必要です。これらは推論にかなり時間がかかることがあります。各ターンには GPUI ハーネスで上限時間があります。`--context-tokens` はローカルモデルのコンテキスト予算（デフォルト65536）を制御しますが、CLI モデルの制限やアプリケーションのデフォルト設定は変えません。

マニフェストは組み込み楽器を要求し、これらの作曲試行では外部音声クリティックを無効にします。クリティックのキャリブレーションは別の実験であるため、利用可能性や提案が最初の作曲比較に影響しないようにします。

各実行では、実行ファイル／DLL のハッシュ、マニフェスト、ソースコミットと Rust 差分、新しい Rust ソースファイル、試行ごとの設定ハッシュ、トランスクリプト、プロジェクトスナップショット、WAV/MIDI アーティファクト、エンコードファイル測定、ツール失敗、経過時間を記録します。ソースの出所は現在のチェックアウトから取得します。ソースを変更する前にビルドしてランナーを起動してください。固定したランタイムを再利用するときは `--source-suite <suite.json>` を渡します。ランナーはランタイムハッシュが同一であることを確認し、バイナリを今日のチェックアウトのものとはせず、元のソース出所を保持します。`source-suite.json` には参照バイト列が固定されるため、実行中のソーススイートが結果を追加して記録済み参照ハッシュを無効にすることはありません。同じモデル名の背後でコントローラーサービスが変わることがあります。タイムスタンプを保持し、可能な限り近いサービス条件で比較してください。

`transport_pass` は、要求したすべてのターンが完了したことを意味します。`delivery_present` は、要求した2つの出力ファイルが存在することを意味します。どちらも音楽的品質や、音楽上の制約が正しいことを示しません。リクエスト、スコア、測定、聴取をすべて確認してください。失敗した試行は比較コホートに残し、音楽的成功率から除外せず、その頻度を報告します。

`listening.html` を開き、評価が完了するまで `private-key.json` を聴取者から遠ざけてください。ページはペアの順序と A/B の身元を無作為化し、評価と根拠を出力します。5つの軸はリクエスト適合度、メロディー、形式、アンサンブルの相互作用、音です。「同点」と「判断できない」は別です。出力は同一の音楽シードではなく、独立したモデル試行です。元のレベルは保持されるため、音量差を明示的に考慮してください。

集計した選好率の前に、ケースごとの結果と失敗数を報告します。3回の反復だけでは、統計的に信頼できる選好の改善は示せません。[evaluation.md](evaluation.ja.md) のシンボリックルーラーと学習済みルーラーは補助的な測定であり、人による聴取やホールドアウト集合に対する最適化の許可の代わりにはなりません。

別の `composition-runtime-smoke-v2.json` マニフェストは、ローカルな変化の正確さ、プレビュー、候補採用、Undo/Redo、配信を検査します。バージョン管理された `melody_six_eight_eight_bars` フィクスチャは、1拍あたりネイティブ960ティックを使い、90 BPM の6/8拍子8小節（16秒）にわたります。品質マニフェスト v1 の元の `melody_six_eight` フィクスチャは、8回の構築反復があるにもかかわらず **4小節のシード**（11520ティック、8秒）です。固定した v1 のペア比較では元のバイト列を保持してください。8小節の伴奏リクエストは、その短いシードを拡張する必要があります。修正版フィクスチャを一方の比較条件に黙って置き換えないでください。

Runtime smoke v1 は、その4小節のシードに現在の8小節を誤って要求していました。その実行は診断用の証拠であり、8小節の有効な配信テストではありません。新しいランタイム検査には v2 を使います。どちらの runtime smoke 版も選好を測定しません。

```sh
uv run --with pytest pytest tools/eval/test_composition_quality.py
uv run --with ruff ruff check tools/eval/composition_quality.py tools/eval/test_composition_quality.py
```

<a id="2026-10-24-implementation-pilot"></a>
## 2026-10-24 実装パイロット

[保持された数値レポート](../tools/eval/references/composition-quality-pilot-2026-10-24.json) は、ベースライン `406277cc` と実装 `da89dc96` を比較し、Codex CLI 0.163.0 と `gpt-6.1-sol` を使っています。この探索的なパイロットでは、大きな実行が進行中の間に開発タスクの最初の2件を選びました。事前登録された有効性研究ではありません。選択した両タスクには条件ごとに3試行があり、選択した試行の省略はありません。4つのホールドアウトタスクは、このパイロットでは評価していません。完全なマニフェストは、より広い開発実行に利用できます。

| 条件 | タスク | WAV/MIDI 付き完了 | 失敗したツール呼び出し（合計） | 経過時間の範囲 | 変更された伴奏を報告した試行 |
| --- | --- | ---: | ---: | ---: | ---: |
| before | loop_title | 3/3 | 10 | 294–334 s | 未報告 |
| before | authored_folk | 3/3 | 9 | 286–345 s | 未報告 |
| after | loop_title | 3/3 | 10 | 198–266 s | 1 |
| after | authored_folk | 3/3 | 6 | 236–296 s | 2 |

失敗したツール呼び出しには、エージェントが完了前に修正した引数も含まれます。経過時間はビルドや他の測定と重なっているため、性能向上として解釈しないでください。12/12 の配信には要求した長さ（タイトルループ19.2秒、フォークアレンジ16秒）があり、12/12 はフルスケールサンプルがゼロでした。外部クリティック呼び出しは0回です。6/6 の folk 試行は、音符とカーブを含む元のメロディークリップ全体を保持しました。

新しい適応レポートは、変更されたバッキングを、明示的なリズム、持続テクスチャー、役割、空の前景によるバイパスから区別します。変更された伴奏は選好スコアではありません。

完全なワークスペースでは doctest を含む4,499件の Rust テストが通過し、Clippy とフォーマットも通過しました。評価スクリプトは10件の pytest ケースと Ruff に合格しました。シンボリックルーラーはバイト単位で同一でした。固定した pop-band レンダーもバイト単位で同一で、TuneJury と MuQ-MuLan の差分はゼロでした。これは回帰チェックであり、新しい作曲がより良いという証拠ではありません。

前景の選択はリクエスト単位です。このパイロットが対象とするのは、明示的な `accompaniment` の生成と再生成であり、これらのクリップの GUI による一般的な再生成ではありません。音楽評価では、`target/quality-evidence/listening/listening.html` に6つの匿名 A/B ペアと評価フォームがあります。別のキーで身元を確認します。元のレベルは保持されています。人による選好評価はまだ収集していないため、音楽的改善は未証明です。その後廃止した批評モデルのキャリブレーション実験でも、その音声モデルが自動的な音楽選択に適することは検証できていません。

<a id="follow-up-validation-of-production-records-and-candidates"></a>
## 本番記録と候補の追跡検証

候補カタログはオプトインのままで、フェーズ方式のプロンプトは既定になりました。[候補ツール](composition-candidates.ja.md) と [制作フェーズ](composition-workflows.ja.md) を参照してください。
