//! Strings that interpolate values.
//!
//! These cannot be [`Key`](crate::Key)s: a translation is not just different words but a
//! different word *order*, so the placeholders have to travel with the sentence. Each message is
//! a function whose body is one `format!` per language, which keeps every format string a
//! literal — the compiler then checks that each translation uses the arguments it was given.

use crate::Language;

macro_rules! messages {
    ($(
        $(#[$doc:meta])*
        fn $name:ident($($arg:ident: $ty:ty),* $(,)?) { en: $en:literal, ja: $ja:literal }
    )*) => {
        $(
            $(#[$doc])*
            pub fn $name(language: Language, $($arg: $ty),*) -> String {
                // `format!` captures the arguments by name from this scope, so a placeholder
                // that does not match a parameter fails to compile in whichever language it
                // appears in.
                match language {
                    Language::English => format!($en),
                    Language::Japanese => format!($ja),
                }
            }
        )*

        #[cfg(test)]
        /// Names of every message, so a test can report on the set as a whole.
        pub(crate) const NAMES: &[&str] = &[$(stringify!($name),)*];
    };
}

messages! {
    /// The engine's line in the status bar.
    fn audio_status(device: &str, rate: f64, channels: usize) {
        en: "Audio: {device} · {rate:.0} Hz · {channels} ch",
        ja: "オーディオ: {device} · {rate:.0} Hz · {channels} ch"
    }

    /// The GPU's tail on the status line.
    fn gpu_in_use(adapter: &str) {
        en: "GPU: {adapter}",
        ja: "GPU: {adapter}"
    }

    /// When no GPU could be opened at all.
    fn gpu_unavailable() {
        en: "GPU: unavailable, using CPU",
        ja: "GPU: 利用不可、CPU を使用"
    }

    /// Heading over the note grid.
    fn piano_roll_title(clip: &str) {
        en: "Piano Roll — {clip}",
        ja: "ピアノロール — {clip}"
    }

    /// The line under the piano roll's heading, naming the gestures that are actually bound.
    fn piano_roll_hint(create: &str, delete: &str) {
        en: "{create}: add note · drag: move · right edge: resize · {delete}: delete · right-click: menu",
        ja: "{create}: ノート追加 · ドラッグ: 移動 · 右端: 長さ変更 · {delete}: 削除 · 右クリック: メニュー"
    }

    /// The same line while the velocity tool is in hand, which binds none of those gestures.
    fn piano_roll_velocity_hint() {
        en: "drag a note up or down: velocity · right-click: menu",
        ja: "ノートを上下にドラッグ: 強さを変更 · 右クリック: メニュー"
    }

    /// Confirmation that a command has been left with no keystroke at all.
    fn binding_unbound(command: &str) {
        en: "“{command}” now has no key.",
        ja: "「{command}」にキーを割り当てないようにしました。"
    }

    /// Which tool the roll has in hand, for the status line when the roll is not on screen.
    fn tool_in_hand(tool: &str) {
        en: "Piano roll tool: {tool}",
        ja: "ピアノロールのツール: {tool}"
    }

    /// What the piano roll says when no clip is selected.
    fn piano_roll_empty(create: &str) {
        en: "Select a MIDI clip to edit its notes ({create} an empty lane to make one)",
        ja: "MIDI クリップを選ぶとノートを編集できます（空のレーンを{create}で新規作成）"
    }

    /// Title of a menu acting on more than one note.
    fn note_count(count: usize) {
        en: "{count} notes",
        ja: "ノート {count} 個"
    }

    /// Title of a menu acting on more than one clip.
    fn clip_count(count: usize) {
        en: "{count} clips",
        ja: "クリップ {count} 個"
    }

    /// Name given to a track the user just created.
    fn new_track_name(number: usize) {
        en: "Track {number}",
        ja: "トラック {number}"
    }

    /// Name given to an audio track the user just created.
    fn new_audio_track_name(number: usize) {
        en: "Audio {number}",
        ja: "オーディオ {number}"
    }

    /// Name given to a clip the user just created.
    fn new_clip_name(number: usize) {
        en: "Clip {number}",
        ja: "クリップ {number}"
    }

    /// Confirmation that a project was written.
    fn saved(path: &str) {
        en: "Saved {path}",
        ja: "{path} を保存しました"
    }

    /// The sheet shown when saving would write over a project that is already there.
    ///
    /// Names the document that would be lost rather than the name that was typed: the system
    /// save dialog already asked about the typed one, and it is not the file that gets written.
    fn would_replace(existing: &str) {
        en: "{existing} already exists. Saving here replaces it, and it cannot be recovered.",
        ja: "{existing} はすでに存在します。ここに保存すると置き換えられ、元に戻すことはできません。"
    }

    /// A project saved, but some of its audio could not be brought into the folder with it.
    fn saved_uncollected(path: &str, count: usize) {
        en: "Saved {path} — {count} audio file(s) stayed outside the folder",
        ja: "{path} を保存しました — {count} 件の音声ファイルはフォルダの外に残りました"
    }

    /// Confirmation that a project was read.
    fn opened(path: &str) {
        en: "Opened {path}",
        ja: "{path} を開きました"
    }

    /// A project opened, but one of its audio files was not where it said.
    fn opened_missing_one(path: &str, missing: &str) {
        en: "Opened {path} — missing audio file {missing}",
        ja: "{path} を開きました — オーディオファイル {missing} が見つかりません"
    }

    /// A project opened, but several of its audio files were missing.
    fn opened_missing_many(path: &str, count: usize) {
        en: "Opened {path} — {count} audio files could not be found",
        ja: "{path} を開きました — オーディオファイル {count} 件が見つかりません"
    }

    /// While a file is being decoded.
    fn importing(path: &str) {
        en: "Importing {path}…",
        ja: "{path} を読み込み中…"
    }

    /// While a project is being read.
    fn opening(path: &str) {
        en: "Opening {path}…",
        ja: "{path} を開いています…"
    }

    /// While a specification is being turned into a piece.
    fn composing(path: &str) {
        en: "Composing from {path}…",
        ja: "{path} から作曲中…"
    }

    /// While a project's files are being gathered into its folder.
    fn collecting() {
        en: "Collecting the project's files…",
        ja: "プロジェクトのファイルを収集中…"
    }

    /// Confirmation that a file was decoded onto a track.
    fn imported(path: &str) {
        en: "Imported {path}",
        ja: "{path} を読み込みました"
    }

    /// Confirmation that a SoundFont was read, and how many sounds it brought with it.
    ///
    /// The count rather than the path, because a font goes on a shelf instead of onto a track:
    /// what a person needs to know next is how much there is to choose from.
    fn soundfont_imported(name: &str, sounds: usize) {
        en: "{name} — {sounds} sound(s) to choose from",
        ja: "{name} — 音色 {sounds} 件から選べます"
    }

    /// Confirmation that a MIDI file was read, and what came out of it.
    ///
    /// Both numbers, because a MIDI file gives no other sign of having worked: a file whose tracks
    /// were all empty and one that was read wrong look identical on an empty timeline.
    fn midi_imported(tracks: usize, notes: usize) {
        en: "Imported {tracks} track(s), {notes} note(s)",
        ja: "{tracks} トラック・{notes} ノートを読み込みました"
    }

    /// Confirmation that a piece left as a MIDI file.
    fn midi_exported(path: &str, notes: usize) {
        en: "Wrote {notes} note(s) to {path}",
        ja: "{notes} 個のノートを {path} に書き出しました"
    }

    /// A file dragged onto the window that no importer here recognises.
    fn cannot_import(path: &str) {
        en: "{path} is not a kind of file Auris can import",
        ja: "{path} は Auris が読み込める種類のファイルではありません"
    }

    /// Confirmation that a drop of several files was read.
    ///
    /// A count rather than a list: the names are already on the tracks the drop just made, and a
    /// status line is one line.
    fn imported_files(count: usize) {
        en: "Imported {count} files",
        ja: "{count} 個のファイルを読み込みました"
    }

    /// A drop where some of the files arrived and some did not.
    fn imported_files_partly(imported: usize, failed: usize) {
        en: "Imported {imported} file(s) · {failed} could not be read",
        ja: "{imported} 個のファイルを読み込みました · {failed} 個は読み込めませんでした"
    }

    /// A drop where nothing at all could be read.
    fn imported_nothing(count: usize) {
        en: "None of the {count} files could be imported",
        ja: "{count} 個のファイルはいずれも読み込めませんでした"
    }

    /// Confirmation that a track's clips will not be written again, and how many that was.
    ///
    /// The count rather than a bare acknowledgement: this acts on clips further down a track than
    /// the panel is showing, and a number is the difference between believing that and checking.
    fn track_kept(count: usize) {
        en: "{count} clip(s) on this track will not be written again",
        ja: "このトラックの {count} 個のクリップは今後書き換えられません"
    }

    /// A project was dragged in alongside other files, and nothing was done.
    ///
    /// Says why rather than only what: opening a project closes the one that is open, so a drop
    /// there is no single reading of is refused whole rather than half-guessed at.
    fn project_wants_to_be_alone() {
        en: "Drop a project on its own — opening one closes the document you have open",
        ja: "プロジェクトは単独でドロップしてください — 開くと今の書類は閉じられます"
    }

    /// Confirmation that everything a project refers to now lives in its folder.
    fn assets_collected(count: usize) {
        en: "Collected {count} file(s) into the project folder",
        ja: "ファイル {count} 件をプロジェクトフォルダに集めました"
    }

    /// Nothing was outside the project folder to begin with.
    fn assets_already_collected() {
        en: "Everything this project uses is already in its folder",
        ja: "このプロジェクトが使うファイルはすべてフォルダ内にあります"
    }

    /// Progress line while a render runs.
    fn rendering(path: &str) {
        en: "Rendering {path}…",
        ja: "{path} を書き出し中…"
    }

    /// Confirmation that a render finished, with what it produced.
    fn exported(path: &str, duration: &str, peak_db: f32) {
        en: "Exported {path} · {duration} · peak {peak_db:.1} dBFS",
        ja: "{path} を書き出しました · {duration} · ピーク {peak_db:.1} dBFS"
    }

    /// A step was undone.
    fn undid(edit: &str) {
        en: "Undid {edit}",
        ja: "{edit}を取り消しました"
    }

    /// A step was redone.
    fn redid(edit: &str) {
        en: "Redid {edit}",
        ja: "{edit}をやり直しました"
    }

    /// Something the user asked for did not work.
    /// `action` is a command's own label, which is why the Japanese quotes it rather than
    /// building a sentence out of it. The labels are a mixture of dictionary-form verbs
    /// (「プロジェクトを開く」), サ変 nouns (「保存」) and noun phrases (「新規プロジェクト」), and no
    /// single frame conjugates all three: 「プロジェクトを開くできませんでした」 is what the old one
    /// produced. Naming the command in quotes is grammatical for every label there is and for
    /// every label anybody adds later, which the alternative was not.
    fn failed(action: &str, reason: &str) {
        en: "Could not {action}: {reason}",
        ja: "「{action}」を実行できませんでした: {reason}"
    }

    /// A device's capabilities, under its name in the settings window.
    fn device_detail(channels: u16, rates: &str) {
        en: "{channels} ch · {rates}",
        ja: "{channels} ch · {rates}"
    }

    /// The span of sample rates a device accepts.
    fn rate_range(low: f64, high: f64) {
        en: "{low:.1}–{high:.1} kHz",
        ja: "{low:.1}–{high:.1} kHz"
    }

    /// A single sample rate.
    fn rate_single(rate: f64) {
        en: "{rate:.1} kHz",
        ja: "{rate:.1} kHz"
    }

    /// A buffer size, with what it costs in latency.
    fn buffer_choice(frames: u32, latency_ms: f64) {
        en: "{frames} · {latency_ms:.1} ms",
        ja: "{frames} · {latency_ms:.1} ms"
    }

    /// What the audio backend is doing right now.
    fn running_device(device: &str, rate: f64, channels: usize, suffix: &str) {
        en: "Running: {device} · {rate:.0} Hz · {channels} ch{suffix}",
        ja: "動作中: {device} · {rate:.0} Hz · {channels} ch{suffix}"
    }

    /// Appended to the line above when no stream is actually open.
    fn silent_suffix() {
        en: " (silent)",
        ja: "（無音）"
    }

    /// Warning that a keystroke is already spoken for.
    fn also_bound_to(command: &str) {
        en: "also {command}",
        ja: "{command}にも割り当て済み"
    }

    /// Confirmation that a command was rebound.
    fn binding_set(command: &str, keystroke: &str) {
        en: "{command} is now {keystroke}",
        ja: "{command} を {keystroke} に割り当てました"
    }

    /// Confirmation that a command was rebound onto a keystroke already in use.
    fn binding_set_with_clash(command: &str, keystroke: &str, other: &str) {
        en: "{command} is now {keystroke} — also bound to {other}",
        ja: "{command} を {keystroke} に割り当てました — {other} にも割り当て済みです"
    }

    /// A keystroke that cannot be bound at all.
    fn binding_rejected(keystroke: &str) {
        en: "{keystroke} cannot be used as a binding",
        ja: "{keystroke} は割り当てに使えません"
    }

    /// An error with a prefix naming the layer it came from.
    fn detailed(prefix: &str, detail: &str) {
        en: "{prefix}: {detail}",
        ja: "{prefix}: {detail}"
    }

    /// A plugin id the registry does not know.
    fn unknown_plugin(id: &str) {
        en: "no plugin is registered as `{id}`",
        ja: "`{id}` というプラグインは登録されていません"
    }

    /// Nothing in the chord-progression catalogue answers to that name.
    fn unknown_progression(name: &str) {
        en: "no chord progression is named `{name}`",
        ja: "`{name}` というコード進行はありません"
    }

    /// A progression was written onto the timeline.
    fn progression_written(name: &str, chords: usize) {
        en: "wrote {chords} chord(s) of `{name}`",
        ja: "`{name}` のコード {chords} 個を書き込みました"
    }

    /// A part was written from the harmony under it.
    fn clip_written(preset: &str, notes: usize) {
        en: "{preset}: {notes} note(s) written",
        ja: "{preset}: {notes} 音を書きました"
    }

    /// Title of the harmony lane's menu, naming the bar it was opened over.
    fn harmony_at_bar(bar: u32) {
        en: "Harmony · bar {bar}",
        ja: "コード進行 · {bar} 小節目"
    }

    /// The window is being drawn in a different colour scheme.
    fn scheme_changed(name: &str) {
        en: "Colour scheme: {name}",
        ja: "カラースキーム: {name}"
    }

    /// Title of the harmony lane's menu, naming the chord it was opened over.
    ///
    /// Both halves: the numeral is what is stored and what the menu edits, and the chord is what
    /// it means in the key it sits in — which is the half a reader recognises.
    fn harmony_chord(numeral: &str, chord: &str) {
        en: "Harmony · {numeral} ({chord})",
        ja: "コード進行 · {numeral}（{chord}）"
    }

    /// One catalogue progression, as a menu row.
    fn write_progression(description: &str) {
        en: "Write {description}",
        ja: "{description} を書き込む"
    }

    /// The text typed into the key prompt is not a key.
    fn not_a_key(text: &str) {
        en: "`{text}` is not a key — try `Eb` or `F# minor`",
        ja: "`{text}` は調ではありません — `Eb` や `F# minor` のように"
    }

    /// The text typed into the chord prompt is not a chord.
    fn not_a_chord(text: &str) {
        en: "`{text}` is not a chord — try `IV`, `vi` or `bVII7`",
        ja: "`{text}` はコードではありません — `IV`、`vi`、`bVII7` のように"
    }

    /// The text typed into the seed prompt is not a number.
    fn not_a_seed(text: &str) {
        en: "`{text}` is not a seed — any whole number will do",
        ja: "`{text}` はシードではありません — 整数を入力してください"
    }

    /// The text typed into the tempo prompt is not a number of beats per minute.
    fn not_a_tempo(text: &str) {
        en: "`{text}` is not a tempo — try `128` or `174.5`",
        ja: "`{text}` はテンポではありません — `128` や `174.5` のように"
    }

    /// The editing grid was stepped to another division.
    fn grid_set(division: &str) {
        en: "Grid: {division}",
        ja: "グリッド: {division}"
    }

    /// The text typed into the time signature prompt is not a meter.
    fn not_a_signature(text: &str) {
        en: "`{text}` is not a time signature — try `4/4` or `6/8`",
        ja: "`{text}` は拍子ではありません — `4/4` や `6/8` のように"
    }

    /// The text typed into the clip gain prompt is not a number of decibels.
    fn not_a_gain(text: &str) {
        en: "`{text}` is not a gain — try `-3` or `2.5`",
        ja: "`{text}` はゲインではありません — `-3` や `2.5` のように"
    }

    /// The text typed into the position prompt is not a bar, beat and hundredth.
    fn not_a_position(text: &str) {
        en: "`{text}` is not a position — try `17` for a bar, or `17.3` for its third beat",
        ja: "`{text}` は位置ではありません — 小節なら `17`、その3拍目なら `17.3` のように"
    }

    /// A project referenced audio files that are not where it said.
    fn missing_audio_files(count: usize) {
        en: "{count} audio file(s) could not be loaded",
        ja: "オーディオファイル {count} 件を読み込めませんでした"
    }

    /// Reopening the audio device did not work.
    fn audio_restart_failed(reason: &str) {
        en: "could not switch audio device: {reason}",
        ja: "オーディオデバイスを切り替えられませんでした: {reason}"
    }

    /// The settings file could not be written.
    fn settings_write_failed(path: &str, reason: &str) {
        en: "could not write {path}: {reason}",
        ja: "{path} を書き込めませんでした: {reason}"
    }

    /// Confirmation that the interface language changed.
    fn language_changed(language_name: &str) {
        en: "Interface language: {language_name}",
        ja: "表示言語: {language_name}"
    }

    /// A command line argument that is not one of ours.
    fn unknown_command(name: &str) {
        en: "unknown command `{name}`; try `auris help`",
        ja: "`{name}` というコマンドはありません。`auris help` を試してください"
    }

    /// A command line option that is not one of ours.
    fn unknown_option(name: &str) {
        en: "unknown option `{name}`",
        ja: "`{name}` というオプションはありません"
    }

    /// An option that needs a value it was not given.
    fn option_needs_value(name: &str, kind: &str) {
        en: "{name} needs {kind}",
        ja: "{name} には{kind}が必要です"
    }

    /// A bit depth the exporter does not offer.
    fn bad_bit_depth(given: &str) {
        en: "--bit-depth wants 16, 24 or 32, not `{given}`",
        ja: "--bit-depth は 16, 24, 32 のいずれかです。`{given}` は使えません"
    }

    /// The progress line a render prints to the terminal.
    fn render_progress(percent: i32) {
        en: "rendering {percent:>3}%",
        ja: "書き出し中 {percent:>3}%"
    }

    /// Confirmation that the command line renderer wrote a file.
    fn wrote_file(path: &str, duration: &str, channels: usize, bits: u16, peak_db: f32) {
        en: "wrote {path} · {duration} · {channels} ch · {bits}-bit · peak {peak_db:.1} dBFS",
        ja: "{path} を書き出しました · {duration} · {channels} ch · {bits} bit · ピーク {peak_db:.1} dBFS"
    }

    /// Confirmation that the command line tool created a project.
    fn created_project(path: &str, bpm: f64, rate: f64) {
        en: "created {path} · {bpm:.2} BPM · {rate:.0} Hz",
        ja: "{path} を作成しました · {bpm:.2} BPM · {rate:.0} Hz"
    }

    /// A file a project refers to that is not there.
    fn warning_missing_audio(path: &str) {
        en: "warning: missing audio file {path}",
        ja: "警告: オーディオファイル {path} が見つかりません"
    }

    /// Confirmation that a piece was composed and written.
    fn composed(path: &str, tracks: usize, notes: usize, seed: u64) {
        en: "composed {path} · {tracks} tracks · {notes} notes · seed {seed}",
        ja: "{path} を作曲しました · {tracks} トラック · {notes} ノート · シード {seed}"
    }

    /// Confirmation that a piece was composed into the open document.
    fn composed_document(tracks: usize, notes: usize, seed: u64) {
        en: "composed {tracks} tracks · {notes} notes · seed {seed}",
        ja: "{tracks} トラック · {notes} ノートを作曲しました · シード {seed}"
    }

    /// The same, with a note that some parts did not get the instrument they asked for.
    fn composed_document_substituted(tracks: usize, notes: usize, seed: u64, missing: usize) {
        en: "composed {tracks} tracks · {notes} notes · seed {seed} · {missing} instruments substituted",
        ja: "{tracks} トラック · {notes} ノートを作曲しました · シード {seed} · {missing} 個の音源を代替しました"
    }

    /// A part asked for an instrument this build does not have.
    fn instrument_substituted(id: &str) {
        en: "warning: no instrument `{id}`; using the default instead",
        ja: "警告: `{id}` という音源がないため既定の音源を使います"
    }

    /// A song specification would not parse.
    fn spec_rejected(path: &str) {
        en: "{path} could not be read as a song specification",
        ja: "{path} を曲の仕様として読めませんでした"
    }

    /// The name a duplicated object takes.
    fn copy_of(name: &str) {
        en: "{name} copy",
        ja: "{name} のコピー"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_message_interpolates_its_arguments_in_both_languages() {
        for language in Language::ALL {
            assert!(saved(language, "song.auris").contains("song.auris"));
            assert!(imported(language, "drums.wav").contains("drums.wav"));
            assert!(note_count(language, 7).contains('7'));
            assert!(exported(language, "a.wav", "00:12.000", -1.25).contains("-1.2"));
            assert!(failed(language, "save", "disk full").contains("disk full"));
            assert!(
                opened_missing_many(language, "song.auris", 3).contains('3'),
                "the count is what the sentence is about"
            );
        }
    }

    #[test]
    fn a_message_reads_differently_in_each_language() {
        // Not every message can differ — some are only numbers and units — but the ones that
        // carry a sentence must, or they were never translated.
        assert_ne!(
            saved(Language::English, "x"),
            saved(Language::Japanese, "x")
        );
        assert_ne!(
            undid(Language::English, "x"),
            undid(Language::Japanese, "x")
        );
    }

    #[test]
    fn the_table_is_not_accidentally_empty() {
        assert!(NAMES.len() > 20, "{} messages", NAMES.len());
    }
}
