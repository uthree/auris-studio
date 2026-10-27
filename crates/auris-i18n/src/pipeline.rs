//! Interpolated copy for the read-only rendering-pipeline inspector.
//!
//! The projection itself belongs to `auris-session`; this module only turns its bounded
//! metadata into words a person can scan. Unknown transform names remain recognizable.

use std::borrow::Cow;

use crate::Language;

/// Formats the number of authored notes in a source node.
pub fn count_notes(language: Language, count: usize) -> String {
    match language {
        Language::English if count == 1 => "1 note".into(),
        Language::English => format!("{count} notes"),
        Language::Japanese => format!("ノート{count}個"),
    }
}

/// Formats the number of points in an automation lane.
pub fn count_points(language: Language, count: usize) -> String {
    match language {
        Language::English if count == 1 => "1 point".into(),
        Language::English => format!("{count} points"),
        Language::Japanese => format!("{count}ポイント"),
    }
}

/// Formats zero-based, half-open page bounds as an inclusive human-readable range.
pub fn page_range(language: Language, start: usize, end: usize, total: usize) -> String {
    match language {
        Language::English => format!("{}–{} of {total}", start.saturating_add(1), end.min(total)),
        Language::Japanese => format!("{}–{} / {total}", start.saturating_add(1), end.min(total)),
    }
}

/// Names one stored clip stage from its zero-based index and user-visible transform name.
pub fn stage_title(language: Language, index: usize, display_name: &str) -> String {
    match language {
        Language::English => format!("Stage {}: {display_name}", index.saturating_add(1)),
        Language::Japanese => format!("ステージ{}：{display_name}", index.saturating_add(1)),
    }
}

/// Returns a localized name for a known `NoteTransform` discriminator.
///
/// Unknown values borrow the discriminator unchanged instead of inventing a label.
pub fn transform_name<'a>(language: Language, kind: &'a str) -> Cow<'a, str> {
    let localized = match (language, kind) {
        (Language::English, "octaves") => "Octaves",
        (Language::Japanese, "octaves") => "オクターブ",
        (Language::English, "pitch") => "Pitch ornaments",
        (Language::Japanese, "pitch") => "ピッチ装飾",
        (Language::English, "expression") => "Expression",
        (Language::Japanese, "expression") => "表情付け",
        (Language::English, "groove") => "Groove",
        (Language::Japanese, "groove") => "グルーヴ",
        (Language::English, "strum") => "Strum",
        (Language::Japanese, "strum") => "ストラム",
        (Language::English, "ghost") => "Ghost notes",
        (Language::Japanese, "ghost") => "ゴーストノート",
        (Language::English, "stroke") => "Stroke",
        (Language::Japanese, "stroke") => "ストローク",
        (Language::English, "mute") => "Mute retrigger",
        (Language::Japanese, "mute") => "ミュート再発音",
        (Language::English, "brush") => "Brush",
        (Language::Japanese, "brush") => "ブラシ",
        (Language::English, "slide") => "Slide",
        (Language::Japanese, "slide") => "スライド",
        (Language::English, "for_drum_voice") => "Drum voice",
        (Language::Japanese, "for_drum_voice") => "ドラム音色",
        (Language::English, "humanize") => "Humanize",
        (Language::Japanese, "humanize") => "ヒューマナイズ",
        (Language::English, "lean") => "Lean",
        (Language::Japanese, "lean") => "前ノリ・後ノリ",
        (Language::English, "swing") => "Swing",
        (Language::Japanese, "swing") => "スウィング",
        (Language::English, "transpose") => "Transpose",
        (Language::Japanese, "transpose") => "移調",
        (Language::English, "gate") => "Gate",
        (Language::Japanese, "gate") => "ゲート",
        _ => return Cow::Borrowed(kind),
    };
    Cow::Borrowed(localized)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn english_counts_and_ranges_are_scannable() {
        assert_eq!(count_notes(Language::English, 12), "12 notes");
        assert_eq!(count_points(Language::English, 8), "8 points");
        assert_eq!(page_range(Language::English, 32, 64, 90), "33–64 of 90");
        assert_eq!(stage_title(Language::English, 1, "Gate"), "Stage 2: Gate");
    }

    #[test]
    fn japanese_counts_and_ranges_keep_the_numbers_visible() {
        assert_eq!(count_notes(Language::Japanese, 12), "ノート12個");
        assert_eq!(count_points(Language::Japanese, 8), "8ポイント");
        assert_eq!(page_range(Language::Japanese, 0, 32, 90), "1–32 / 90");
        assert_eq!(
            stage_title(Language::Japanese, 1, "ゲート"),
            "ステージ2：ゲート"
        );
    }

    #[test]
    fn transform_names_localize_known_kinds_and_preserve_unknown_values() {
        assert_eq!(transform_name(Language::English, "gate"), "Gate");
        assert_eq!(transform_name(Language::Japanese, "gate"), "ゲート");
        assert_eq!(
            transform_name(Language::Japanese, "future_stage"),
            "future_stage"
        );
    }
}
