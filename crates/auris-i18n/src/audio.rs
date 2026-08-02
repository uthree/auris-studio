//! Translations for the words plugins use.
//!
//! Plugin metadata belongs to the plugin, not to this crate: a third-party instrument will never
//! appear in any table here, and its parameter names still have to be shown. So these are
//! *lookups keyed by the English term* with a fallback to that term, rather than an enum.
//!
//! Keying on the English word rather than on `(plugin id, parameter key)` is deliberate. Audio
//! vocabulary is shared — every compressor's "Attack" is the same idea — so one entry translates
//! the word everywhere it appears, including in plugins written later by someone else.

use crate::Language;

/// Translation of a plugin's display name, or `name` itself when it is not known here.
pub fn plugin_name(name: &str, language: Language) -> &str {
    lookup(PLUGIN_NAMES, name, language)
}

/// Translation of a plugin's one-line description, or `text` itself when it is not known here.
pub fn plugin_description(text: &str, language: Language) -> &str {
    lookup(PLUGIN_DESCRIPTIONS, text, language)
}

/// Translation of a parameter's display name, or `name` itself when it is not known here.
pub fn parameter(name: &str, language: Language) -> &str {
    lookup(PARAMETERS, name, language)
}

/// Translation of one option of a choice parameter, or `label` itself when unknown.
pub fn choice(label: &str, language: Language) -> &str {
    lookup(CHOICES, label, language)
}

/// Translation of a plugin category, or `label` itself when it is not known here.
pub fn category(label: &str, language: Language) -> &str {
    lookup(CATEGORIES, label, language)
}

/// Whether `term` appears in any table here.
///
/// Distinct from "the translation differs": a handful of audio terms — `Q`, `dB` — are the same
/// word in Japanese, so a completeness check has to ask whether the term was *considered*, not
/// whether it changed.
pub fn is_known(term: &str) -> bool {
    [
        PLUGIN_NAMES,
        PLUGIN_DESCRIPTIONS,
        PARAMETERS,
        CHOICES,
        CATEGORIES,
    ]
    .into_iter()
    .any(|table| table.iter().any(|(english, _)| *english == term))
}

/// Finds `term` in `table`, falling back to the term itself.
///
/// The fallback is the whole point: English is the canonical label a plugin ships with, so an
/// untranslated term shows as its author wrote it rather than as a missing-string marker.
fn lookup<'a>(
    table: &[(&'static str, &'static str)],
    term: &'a str,
    language: Language,
) -> &'a str {
    match language {
        Language::English => term,
        Language::Japanese => table
            .iter()
            .find(|(english, _)| *english == term)
            .map(|(_, translated)| *translated)
            .unwrap_or(term),
    }
}

/// Japanese names for the built-in plugins.
const PLUGIN_NAMES: &[(&str, &str)] = &[
    ("Chiptune", "チップチューン"),
    ("FM 2-Op", "FM 2 オペレーター"),
    ("Noise Drum", "ノイズドラム"),
    ("Compressor", "コンプレッサー"),
    ("Delay", "ディレイ"),
    ("Distortion", "ディストーション"),
    ("Equalizer", "イコライザー"),
    ("Gain & Pan", "ゲイン & パン"),
    ("Limiter", "リミッター"),
    ("Reverb", "リバーブ"),
];

/// Japanese versions of the one-line descriptions shown in the plugin browser.
const PLUGIN_DESCRIPTIONS: &[(&str, &str)] = &[
    (
        "Band-limited pulse, saw, triangle and LFSR noise with unison and bit crushing",
        "帯域制限したパルス・ノコギリ・三角波と LFSR ノイズ。ユニゾンとビットクラッシュ付き",
    ),
    (
        "Two-operator phase modulation: bells, basses and electric pianos",
        "2 オペレーターの位相変調。ベル、ベース、エレピ向き",
    ),
    (
        "Pitch-swept band-passed LFSR noise: kicks, snares and hats without a sampler",
        "ピッチスイープとバンドパスを掛けた LFSR ノイズ。サンプラー無しでキック・スネア・ハット",
    ),
    (
        "Soft-knee compressor with stereo-linked peak detection",
        "ソフトニー、ステレオリンクのピーク検出コンプレッサー",
    ),
    (
        "Feedback delay with damping and a ping-pong mode",
        "ダンピングとピンポンモードを備えたフィードバックディレイ",
    ),
    (
        "Saturation, hard clipping, wave folding and bitcrushing",
        "サチュレーション、ハードクリップ、ウェーブフォールド、ビットクラッシュ",
    ),
    (
        "Six band EQ: high-pass, low shelf, two bells, high shelf, low-pass",
        "6 バンド EQ。ハイパス、ローシェルフ、ベル 2 基、ハイシェルフ、ローパス",
    ),
    (
        "Level, stereo position, stereo width and polarity",
        "レベル、定位、ステレオ幅、位相",
    ),
    (
        "Look-ahead brickwall limiter with a guaranteed output ceiling",
        "先読み型のブリックウォールリミッター。出力上限を保証します",
    ),
    (
        "Schroeder reverb: eight damped combs into four all-passes per channel",
        "シュレーダー型リバーブ。1 チャンネルあたり減衰コム 8 基とオールパス 4 基",
    ),
];

/// Japanese names for parameters. Shared across plugins on purpose — see the module note.
const PARAMETERS: &[(&str, &str)] = &[
    ("Attack", "アタック"),
    ("Bit Depth", "ビット深度"),
    ("Ceiling", "上限"),
    ("Damping", "ダンピング"),
    ("Decay", "ディケイ"),
    ("Detune", "デチューン"),
    ("Drive", "ドライブ"),
    ("Feedback", "フィードバック"),
    ("Gain", "ゲイン"),
    ("Glide", "グライド"),
    ("Index", "変調指数"),
    ("Input", "入力"),
    ("Knee", "ニー"),
    ("Level", "レベル"),
    ("Makeup", "メイクアップ"),
    ("Mix", "ミックス"),
    ("Mod Decay", "変調ディケイ"),
    ("Mode", "モード"),
    ("Octave", "オクターブ"),
    ("Output", "出力"),
    ("Pan", "パン"),
    ("Phase Invert", "位相反転"),
    ("Ping-Pong", "ピンポン"),
    ("Pitch Sweep", "ピッチスイープ"),
    ("Pulse Width", "パルス幅"),
    ("Ratio", "レシオ"),
    ("Release", "リリース"),
    ("Room Size", "ルームサイズ"),
    ("Steps", "ステップ数"),
    ("Sustain", "サステイン"),
    ("Tone", "トーン"),
    ("Unison", "ユニゾン"),
    ("Unison Spread", "ユニゾン幅"),
    ("Waveform", "波形"),
    ("Width", "ステレオ幅"),
    ("Threshold", "スレッショルド"),
    ("Time", "タイム"),
    ("Pre-Delay", "プリディレイ"),
    // The equaliser names its bands by abbreviation, which stays an abbreviation in Japanese.
    // "Q" is "Q" in Japanese as well; the entries exist so the completeness test can tell the
    // difference between a term that was considered and one that was forgotten.
    ("HP Freq", "HP 周波数"),
    ("HP Gain", "HP ゲイン"),
    ("HP On", "HP 有効"),
    ("HP Q", "HP Q"),
    ("LS Freq", "LS 周波数"),
    ("LS Gain", "LS ゲイン"),
    ("LS On", "LS 有効"),
    ("LS Q", "LS Q"),
    ("P1 Freq", "P1 周波数"),
    ("P1 Gain", "P1 ゲイン"),
    ("P1 On", "P1 有効"),
    ("P1 Q", "P1 Q"),
    ("P2 Freq", "P2 周波数"),
    ("P2 Gain", "P2 ゲイン"),
    ("P2 On", "P2 有効"),
    ("P2 Q", "P2 Q"),
    ("HS Freq", "HS 周波数"),
    ("HS Gain", "HS ゲイン"),
    ("HS On", "HS 有効"),
    ("HS Q", "HS Q"),
    ("LP Freq", "LP 周波数"),
    ("LP Gain", "LP ゲイン"),
    ("LP On", "LP 有効"),
    ("LP Q", "LP Q"),
];

/// Japanese names for the browser's category headings.
const CATEGORIES: &[(&str, &str)] = &[
    ("Synth", "シンセ"),
    ("Sampler", "サンプラー"),
    ("Drum", "ドラム"),
    ("Equalizer", "イコライザー"),
    ("Dynamics", "ダイナミクス"),
    ("Delay", "ディレイ"),
    ("Reverb", "リバーブ"),
    ("Distortion", "ディストーション"),
    ("Modulation", "モジュレーション"),
    ("Utility", "ユーティリティ"),
    ("Other", "その他"),
];

/// Japanese labels for the options of choice parameters.
const CHOICES: &[(&str, &str)] = &[
    ("Sine", "サイン波"),
    ("Square", "矩形波"),
    ("Saw", "ノコギリ波"),
    ("Triangle", "三角波"),
    ("Noise", "ノイズ"),
    ("Soft (tanh)", "ソフト (tanh)"),
    ("Hard clip", "ハードクリップ"),
    ("Fold", "フォールド"),
    ("Bitcrush", "ビットクラッシュ"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_term_is_translated_and_an_unknown_one_is_not() {
        assert_eq!(parameter("Attack", Language::Japanese), "アタック");
        assert_eq!(parameter("Attack", Language::English), "Attack");
        assert_eq!(
            parameter("Warp Factor", Language::Japanese),
            "Warp Factor",
            "a plugin we have never heard of must still show its own label"
        );
    }

    #[test]
    fn every_table_entry_says_something() {
        for table in [
            PLUGIN_NAMES,
            PLUGIN_DESCRIPTIONS,
            PARAMETERS,
            CHOICES,
            CATEGORIES,
        ] {
            for (english, japanese) in table {
                assert!(!english.is_empty() && !japanese.is_empty());
                assert!(is_known(english));
            }
        }
    }

    #[test]
    fn only_abbreviations_are_left_as_they_are() {
        // An entry identical in both languages is nearly always a forgotten translation, so the
        // ones that are genuinely the same word are the ones ending in an abbreviation.
        for (english, japanese) in PARAMETERS {
            if english == japanese {
                assert!(
                    english.ends_with(" Q"),
                    "`{english}` is identical in both languages"
                );
            }
        }
    }

    #[test]
    fn no_english_term_appears_twice() {
        // A second entry for a term would be dead: the lookup stops at the first match, so the
        // duplicate silently never applies.
        for table in [
            PLUGIN_NAMES,
            PLUGIN_DESCRIPTIONS,
            PARAMETERS,
            CHOICES,
            CATEGORIES,
        ] {
            let mut terms: Vec<&str> = table.iter().map(|(english, _)| *english).collect();
            terms.sort_unstable();
            let before = terms.len();
            terms.dedup();
            assert_eq!(before, terms.len(), "a term is listed twice");
        }
    }
}
