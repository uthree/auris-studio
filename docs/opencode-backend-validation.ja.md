<a id="opencode-backend-validation"></a>
# OpenCodeバックエンド検証

[English](opencode-backend-validation.md)

2026-10-24の試行では、インストール済みOpenCode 2.1.3 CLI、実モデル推論、実GPUIウィンドウハーネス、認証済みライブMCP、通常のセッションとレンダラーを使いました。曲と外部ファイルはすべて一時フィクスチャです。ウィンドウハーネスにディスプレイやオーディオデバイスは不要です。作曲ライター、楽器定数、DSPアルゴリズムは変更していません。

<a id="repairs-found-through-inference"></a>
## 推論で見つかった修正

- 実際のV2推論はネイティブ `shell` ツールを要求しましたが、V1専用転送は拒否しました。起動前にCLIのメジャーバージョンを検出し、レガシー設定ではなく現行スキーマを生成します。V2は `agents`／`system`／`permissions`、V1は `agent`／`prompt`／`permission`／`tools` を使います。V2の `shell` 名を認識します。
- `codemode: false` でV2 MCPツールを直接利用可能にします。トークンはプライベート設定が参照する子プロセス環境変数で渡します。プロセスとACPのプロジェクトディレクトリをそろえ、推論前に認証済みツールカタログの要求を待ち、`auris_live_` 接頭辞の意味を説明します。これにより、`session/new` 成功だけを利用可能なツール接続と誤認しません。
- ACPツール通知は `cwd` だけで始まり、表示タイトルが変わることがあります。安定した `name` を優先し、引数の更新を保持し、初期の部分入力を完全なコマンドとして記録せず、実行開始または完了時に呼び出しを出します。

共有MCPテストはカタログ準備とキャンセルを確認します。スクリプト化ACP回帰は両スキーマ、安定名、引数蓄積、モデル選択、ネイティブ許可／拒否、レビュー分離、メモリを対象にします。Windowsのバージョン検査はACP子プロセスと同じ隠しコンソール動作を使います。これらのmacOS試行では、インストール済みCLIでV1とWindowsを実行していません。

<a id="actual-trials"></a>
## 実際の試行

標準編集、アレンジ、呼吸、ワーカーのライフサイクルは `opencode/deepseek-v4.1-flash-free`、厳格な作曲は `opencode/glm-5.3-flash-free`、画像参照は `opencode/nemotron-3.5-super-vl-free` を使いました。音声批評モデルは `127.0.0.1:11434` のローカルOllama `gemma4:e2b` です。

| 試行 | 結果 | 証拠 |
| --- | --- | --- |
| 未保存編集とエクスポート | 52.26秒で合格 | 正確なノート／ミキサー／CC編集、他ノートとCC7保持、Undo/Redo、MIDI往復、48 kHzステレオ24ビットWAV、フルスケールなし |
| アレンジ、演奏、オートメーション、SFX | 100.29秒で合格 | 音色変更／ストラム、範囲付きオートメーション、SFXとライザー、独立読み戻し、ペア音声、Undo/Redo |
| メロディー呼吸 | 82.40秒で合格 | 正確な終端修復、保護参照、前後ステム／ミックス検査、読み戻し、Undo/Redo |
| 権限、テキスト、履歴、停止 | 39.43秒で合格 | 拒否ファイル不在、承認ファイル一致、ワーカー交代後のタグ、未対応圧縮の明示、保留コマンドのキャンセル |
| 画像と音声参照 | 11.57秒で合格 | 画像の色、2.0秒、ローカル音声メタデータの `audio_sent: false` |
| GLM作曲とA/B聴取 | 243.77秒で合格 | 8小節、3トラック、159ノート、2回の音声送信、ベース編集、ミキサー読み戻し、MIDIと24ビットWAV |
| DeepSeek作曲とA/B聴取 | 厳格ハーネスは失敗（132.77秒） | 同じ曲と音声送信／エクスポートは成功したが、比較前の独立ミキサー読み戻しを省略 |

アレンジと呼吸では、それぞれ2回と1回の引数拒否がありました。モデルは修正して状態保持検査に合格しました。これは回復したエラーとして記録しています。

GLM曲はChiptune、Physical Bass、ポリフォニックPhysical Drumsを使い、ドラム役割を生成前に割り当てました。各クリップは明示的シード（101、102、103）で `generate_phrase` を使い、44、20、95ノートを含みます。MIDIは159 Note Onイベントを独立に解析しました。16秒・48 kHzステレオWAVは768,000フレーム、24ビットPCM、WAVE_FORMAT_EXTENSIBLE、−17.749 LUFS、フルスケールなしでした。`verify_render` はPASSを返し、SHA-256は `7a4a61bd0b46e8132f1b2bf0b01891cc2bfe757cb6df4bc8a9654f625745deac` です。

最初の試聴はリビジョン46の1〜4小節でした。モデルはベースを−12 dBから−15 dBに下げ、ミキサーを独立に読み戻してから、保持した試聴IDでリビジョン47の同じ小節を比較しました。両抜粋の保護減衰は0 dBで、ピークは−11.695／−11.786 dBFS、RMSは−20.984／−20.991 dBFSでした。現在のWAVハッシュは基準と異なります。ホストはエージェント終了後に最終プロジェクトを保存しました。

DeepSeek試行は送信、編集、エクスポートを証明しますが、厳格な聴取ループ合格には数えません。GLMリクエストはset-strip、独立ミキサー読み、比較を順番に要求しており、書き込み応答内のグラフは独立読みになりません。批評モデルは誤る可能性があり、focusは意図したベース変更を明かしました。これらはツール実行と測定音声差の証拠であり、信頼できる聴覚識別、音楽的改善、人間のスピーカー聴取の証明ではありません。

最終検査は `auris-agent` の既定128件と選択したヘッドレスGPUIエージェント／設定テスト128件に合格しました。既定実行では各9件のオプトインテストを無視し、実推論は別に実行しました。`cargo fmt --all -- --check`、`cargo clippy --workspace --all-targets`、`git diff --check` も合格しました。

機械可読結果は [`opencode-backend-2026-10-24.json`](../tools/eval/references/opencode-backend-2026-10-24.json) にあります。生トランスクリプト、生成プロジェクト、MIDI、WAVは無視された `.local-evaluations/opencode-2026-10-24/` に残ります。

<a id="automatic-review-limitation"></a>
## 自動レビューの制限

別セッションでは承認レビューのため全ツールを拒否します。実試行ではOpenCodeの無料プロバイダーが `auth_required` でツールなしリクエストを拒否し、無料枠のクライアント制限と診断しました。DeepSeekとGLMの無料エンドポイントは同じ動作でした。GUIは手動確認に戻り、保留操作を実行しませんでした。テストは自己承認せず停止しました。この環境の無料エンドポイントでは自動レビューを利用できません。ツールなしレビューを受け付けるプロバイダー／モデルを使い、資格情報はOpenCodeに管理させてください。

完成した作曲／聴取試行は使い捨て新規ドキュメントに `edit_project.*` を明示的に許可します。外部ネイティブ操作はAutoReviewを使います。この規則はレポートに記録され、製品の既定値は変わりません。無料プロバイダーの自動レビュー成功を示すものではありません。

<a id="reproduction"></a>
## 再現

オプトインGPUIテストは、次のJSONを指す `AURIS_TOOL_SURFACE_EVAL` を受け取ります。

```json
{
  "backend": "opencode",
  "model": "opencode/deepseek-v4.1-flash-free",
  "output": "/absolute/path/to/a/new/evidence/directory",
  "timeout_seconds": 420
}
```

標準編集／エクスポート、アレンジ、メロディー呼吸には `ui::agent_chat::tool_surface_eval` の該当テストを実行します。アレンジと呼吸には `AURIS_ARRANGEMENT_HARNESS=1` も必要です。

独立ワーカーテスト `backends::opencode::evaluation::installed_opencode_checks_native_permissions_references_memory_and_stop` は `AURIS_OPENCODE_EVAL_MODEL` と新しい絶対 `AURIS_OPENCODE_EVAL_OUTPUT` を使います。ネイティブ拒否／承認、テキスト添付内容、交代ワーカーの会話メモリ、未対応手動圧縮、ファイルを作らない保留権限中のStopを確認します。

別の無視テスト `installed_opencode_reads_image_and_audio_references` は、標準エクスポートの48 kHz・2秒WAVを指す `AURIS_OPENCODE_EVAL_AUDIO` も受け取ります。視覚対応コントローラーを選び、生成画像の色、音声長、`audio_sent: false` を確認します。この添付経路はローカル測定値を渡します。`listen` は音声入力を受け付けるモデルにしか使えないため、OpenCode には提示しません。
