//! Strings that interpolate values.
//!
//! These cannot be [`Key`](crate::Key)s: a translation is not just different words but a
//! different word *order*, so the placeholders have to travel with the sentence. Most of them are
//! one line in each file under `locales/`, and the build script writes each as a function whose
//! body is one `format!` per language; the generator checks that every translation names the same
//! placeholders, and the compiler then checks the format strings.
//!
//! The functions written here by hand are the ones a file cannot hold: a sentence chosen by a
//! flag or assembled from optional parts, one without any value, and one whose parameters are
//! declared in another order than the English sentence reads them (the generator takes the
//! order from the sentence, and swapping two counts at every call site would not be caught).

pub use crate::generated::messages::*;

use crate::Language;

/// Explain an exhausted Auris tool allowance and the recovery action for this surface.
pub fn agent_tool_call_limit(language: Language, limit: usize, workshop: bool) -> String {
    match (language, workshop) {
        (Language::English, false) => format!(
            "Stopped after {limit} Auris tool calls. Completed edits remain in the project. Increase Tool calls per request in Settings > Agent, then send a new instruction to continue."
        ),
        (Language::English, true) => format!(
            "Reached {limit} Auris tool calls while creating this candidate. Progress is saved. Increase Tool calls per request in Settings > Agent, then choose Resume recovery to continue."
        ),
        (Language::Japanese, false) => format!(
            "Aurisのツール呼び出しが上限の{limit}回に達したため停止しました。完了した編集はプロジェクトに残っています。「設定 → エージェント」で呼び出し上限を増やし、新しい指示で続きを依頼できます。"
        ),
        (Language::Japanese, true) => format!(
            "この曲案の作成中に、Aurisのツール呼び出しが上限の{limit}回に達しました。途中までの作業は保存されています。「設定 → エージェント」で呼び出し上限を増やし、「途中から復旧」で続けられます。"
        ),
    }
}

/// Play or release one test note in the physical instrument editor.
pub fn physical_preview(language: Language, note: &str, playing: bool) -> String {
    match (language, playing) {
        (Language::English, false) => format!("Play {note}"),
        (Language::English, true) => format!("Release {note}"),
        (Language::Japanese, false) => format!("{note}を試奏"),
        (Language::Japanese, true) => format!("{note}を離す"),
    }
}

/// Performance controls and voice count beside a live mechanical projection.
pub fn physical_activity(
    language: Language,
    voices: usize,
    expression: f32,
    pressure: Option<f32>,
    pedal: Option<bool>,
) -> String {
    let shown = voices.min(4);
    match language {
        Language::English => format!(
            "{shown}/{voices} voices · Expression {:.0}%{}{}",
            expression * 100.,
            pressure.map_or_else(String::new, |value| format!(
                " · Bow pressure {:.0}%",
                value * 100.
            )),
            pedal.map_or_else(String::new, |on| format!(
                " · Pedal {}",
                if on { "on" } else { "off" }
            ))
        ),
        Language::Japanese => format!(
            "{voices}音中{shown}音を表示 · 強弱 {:.0}%{}{}",
            expression * 100.,
            pressure.map_or_else(String::new, |value| format!(" · 弓圧 {:.0}%", value * 100.)),
            pedal.map_or_else(String::new, |on| format!(
                " · ペダル{}",
                if on { "あり" } else { "なし" }
            ))
        ),
    }
}

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

    };
}

messages! {
    /// Counts enabled inserts without hiding bypassed slots from the inspector summary.
    fn inspector_effects(enabled: usize, total: usize) {
        en: "{enabled}/{total} active",
        ja: "{total}件中{enabled}件が有効"
    }











    /// When no GPU could be opened at all.
    fn gpu_unavailable() {
        en: "GPU: unavailable, using CPU",
        ja: "GPU: 利用不可、CPU を使用"
    }


    /// The line under the piano roll's heading, naming the gestures that are actually bound.
    /// `alt` is the platform's own name for the key: Option on a Mac, Alt elsewhere.
    fn piano_roll_hint(create: &str, delete: &str, alt: &str) {
        en: "{create}: add note · drag: move · {alt}-drag: copy · right edge: resize · {delete}: delete · right-click: menu",
        ja: "{create}: ノート追加 · ドラッグ: 移動 · {alt}+ドラッグ: 複製 · 右端: 長さ変更 · {delete}: 削除 · 右クリック: メニュー"
    }





    /// The same line while the velocity tool is in hand, which binds none of those gestures.
    fn piano_roll_velocity_hint() {
        en: "drag a note up or down: velocity · right-click: menu",
        ja: "ノートを上下にドラッグ: 強さを変更 · 右クリック: メニュー"
    }










    /// The same row for a clip that has never been told what tempo it was played at.
    fn clip_source_tempo_unknown() {
        en: "Recorded Tempo: not set…",
        ja: "元のテンポ: 未設定…"
    }



























    /// While a project's files are being gathered into its folder.
    fn collecting() {
        en: "Collecting the project's files…",
        ja: "プロジェクトのファイルを収集中…"
    }










    /// Confirmation that a singer track's features left as a file.
    fn frames_exported(path: &str, frames: usize) {
        en: "wrote {frames} frames to {path}",
        ja: "{path} に {frames} フレームを書き出しました"
    }









    /// Confirmation that a piece left as a MIDI file.
    fn midi_exported(path: &str, notes: usize) {
        en: "Wrote {notes} note(s) to {path}",
        ja: "{notes} 個のノートを {path} に書き出しました"
    }






    /// A project was dragged in alongside other files, and nothing was done.
    ///
    /// Says why rather than only what: opening a project closes the one that is open, so a drop
    /// there is no single reading of is refused whole rather than half-guessed at.
    fn project_wants_to_be_alone() {
        en: "Drop a project on its own — opening one closes the document you have open",
        ja: "プロジェクトは単独でドロップしてください — 開くと今の書類は閉じられます"
    }


    /// Nothing was outside the project folder to begin with.
    fn assets_already_collected() {
        en: "Everything this project uses is already in its folder",
        ja: "このプロジェクトが使うファイルはすべてフォルダ内にあります"
    }















    /// Appended to the line above when no stream is actually open.
    fn silent_suffix() {
        en: " (silent)",
        ja: "（無音）"
    }








    /// A progression was written onto the timeline.
    fn progression_written(name: &str, chords: usize) {
        en: "wrote {chords} chord(s) of `{name}`",
        ja: "`{name}` のコード {chords} 個を書き込みました"
    }

    /// An accompaniment was written around a melody.
    ///
    /// The key leads, because it is the guess everything else rests on: somebody who does not
    /// recognise it has just found out why the rest sounds wrong, without having to go looking.
    fn accompaniment_written(key: &str, parts: usize, chords: usize) {
        en: "Read as {key}: {chords} chord(s), {parts} part(s)",
        ja: "{key} と判断しました: コード {chords} 個、パート {parts} 個"
    }



































    /// A one-bar count-in, where English will not have "1 bars".
    fn count_in_one_bar() {
        en: "1 bar",
        ja: "1 小節"
    }









    /// A take that produced nothing, which reads as a failure until it is explained.
    fn recorded_nothing() {
        en: "the take was empty; nothing was kept",
        ja: "音が入っていなかったため、何も保存しませんでした"
    }



    /// Several tracks recorded at once, which is one press of Record and several takes.
    ///
    /// The count rather than the names: four tracks named in a status line is a line nobody can
    /// read at a glance, and the clips that just appeared are on screen saying which they were.
    fn recorded_takes(tracks: usize, duration: &str) {
        en: "recorded {duration} on {tracks} tracks",
        ja: "{tracks} トラックに {duration} を録音しました"
    }

    /// Several tracks at once, with a hole in all of them.
    ///
    /// One pool feeds every file, so a block the disk could not keep up with is missing from all
    /// of them at the same moment.
    fn recorded_takes_with_gaps(tracks: usize, duration: &str, frames: u64) {
        en: "recorded {duration} on {tracks} tracks, but {frames} frames were dropped — there is a gap in each",
        ja: "{tracks} トラックに {duration} を録音しましたが、{frames} フレームが欠落しています"
    }


    /// A take that was played but fell outside the punch region.
    ///
    /// A different thing from an empty take, and it has to read as one: nothing was wrong with the
    /// microphone, so "nothing was recorded" would send somebody to check their cable.
    fn recorded_outside_punch() {
        en: "nothing was played inside the punch region; the take was kept as a file",
        ja: "パンチ範囲内に演奏がありませんでした。録音はファイルとして残してあります"
    }






    /// The same note for a file that predates the record of who saved it.
    fn warning_saved_by_older_build() {
        en: "warning: saved by an older build; regenerating writes in this build's style",
        ja: "警告: 以前のバージョンで保存されたプロジェクトです。再生成は現在のスタイルで書き直します"
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
            assert!(recovery_from(language, "song.auris").contains("song.auris"));
            assert!(recovered_missing_many(language, "Song", 3).contains('3'));
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
}
