//! What the MIDI controllers are called.
//!
//! A number is what the wire carries and what the document stores — see
//! [`ClipCurve::Controller`](https://docs.rs/auris-core) — and a number is not what anybody wants
//! to read off a lane. This is the table that turns 11 into "Expression", and the list of the ones
//! worth offering in a menu.
//!
//! Not every controller is here and none of them has to be: MIDI defines a hundred and twenty-eight
//! and gives about twenty of them an agreed meaning, so an unnamed one is shown as `CC 85` rather
//! than invented a name for. A file that used it meant something by it, and guessing what would be
//! worse than saying which number it was.

use crate::Language;

/// The controllers a lane can be opened on from the menu, in the order it lists them.
///
/// The performance controls a keyboard actually has, roughly in the order they sit under a hand:
/// the two wheels, the pedals a foot reaches, the two levels, and the filter. Anything else a
/// piece uses arrives by way of a MIDI file, and the menu offers those too — beside these, and
/// marked, because a lane the file already wrote on is the one somebody is looking for.
pub const NOTABLE: [u8; 8] = [1, 2, 4, 7, 10, 11, 64, 74];

/// What controller `number` is called, or `None` when it has no agreed name.
pub fn controller_name(number: u8, language: Language) -> Option<&'static str> {
    NAMES
        .iter()
        .find(|(known, _, _)| *known == number)
        .map(|(_, english, japanese)| match language {
            Language::English => *english,
            Language::Japanese => *japanese,
        })
}

/// What a lane on `number` is labelled: its name where it has one, `CC 85` where it does not.
pub fn controller_label(number: u8, language: Language) -> String {
    match controller_name(number, language) {
        Some(name) => name.to_string(),
        None => format!("CC {number}"),
    }
}

/// Number, English, Japanese. The meanings MIDI itself agrees on, and no more.
const NAMES: &[(u8, &str, &str)] = &[
    (1, "Modulation", "モジュレーション"),
    (2, "Breath", "ブレス"),
    (4, "Foot", "フットコントローラー"),
    (5, "Portamento Time", "ポルタメントタイム"),
    (7, "Volume", "ボリューム"),
    (10, "Pan", "パン"),
    (11, "Expression", "エクスプレッション"),
    (64, "Sustain", "サステインペダル"),
    (65, "Portamento", "ポルタメント"),
    (66, "Sostenuto", "ソステヌート"),
    (67, "Soft Pedal", "ソフトペダル"),
    (71, "Resonance", "レゾナンス"),
    (72, "Release Time", "リリースタイム"),
    (73, "Attack Time", "アタックタイム"),
    (74, "Brightness", "ブライトネス"),
    (84, "Portamento Control", "ポルタメントコントロール"),
    (91, "Reverb Send", "リバーブセンド"),
    (93, "Chorus Send", "コーラスセンド"),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_named_controller_is_read_by_its_name_and_the_rest_by_their_number() {
        assert_eq!(
            controller_label(1, Language::Japanese),
            "モジュレーション",
            "the wheel everybody knows"
        );
        assert_eq!(controller_label(11, Language::English), "Expression");
        // A controller MIDI has no meaning for keeps its number, in both languages: the file that
        // used it meant something by it, and a name invented here would be a different claim.
        assert_eq!(controller_label(85, Language::English), "CC 85");
        assert_eq!(controller_label(85, Language::Japanese), "CC 85");
        assert_eq!(controller_name(85, Language::English), None);
    }

    #[test]
    fn every_controller_the_menu_offers_has_a_name_to_offer_it_under() {
        // A menu row reading "CC 74" is a row nobody picks. Anything in the list has to be a
        // control somebody can recognise, in both languages.
        for number in NOTABLE {
            for language in [Language::English, Language::Japanese] {
                assert!(
                    controller_name(number, language).is_some(),
                    "controller {number} is offered unnamed in {language:?}"
                );
            }
        }
    }

    #[test]
    fn the_table_names_each_controller_once_and_in_number_order() {
        // Read by a linear search and shown as a list; a duplicate would make the second entry
        // unreachable and a number out of order would put the menu in a jumble.
        for pair in NAMES.windows(2) {
            assert!(
                pair[0].0 < pair[1].0,
                "{} and {} are out of order",
                pair[0].0,
                pair[1].0
            );
        }
    }
}
