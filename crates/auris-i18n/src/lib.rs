//! Every word Auris Studio shows a person, in every language it speaks.
//!
//! # Why the text lives in its own crate
//!
//! Both frontends display text, and neither should own the translations — a string the desktop
//! application and the command line tool both need would otherwise exist twice and drift. This
//! crate has no dependencies inside the workspace, so anything that talks to a person can use it
//! without pulling the engine in behind it.
//!
//! # Shape
//!
//! The words are not in Rust at all. `locales/en.toml` and `locales/ja.toml` hold them, one file
//! per language, and the build script runs them through [`atelier_i18n`]'s generator, which
//! writes [`Language`], [`Key`] and the functions in [`messages`]:
//!
//! ```
//! # use auris_i18n::{Key, Language};
//! assert_eq!(Key::Mixer.get(Language::English), "Mixer");
//! assert_eq!(Key::Mixer.get(Language::Japanese), "ミキサー");
//! ```
//!
//! A string without a placeholder is a [`Key`] variant, named as the file writes it. A string with
//! `{named}` placeholders is a function in [`messages`], whose parameters follow the order the
//! English text uses them. A key that one file has and the other lacks, or a translation that
//! names different placeholders, fails the build with the file and the key — so a language that is
//! missing a string is still a compile error rather than a hole discovered by a user. Adding a
//! language is adding a file, and the build then lists every string it still needs.
//!
//! What a file cannot say stays in Rust, in [`messages`]: a sentence chosen by a flag or built
//! from optional parts, and the few whose parameters are declared in another order than the
//! English sentence reads them.
//!
//! Plugin metadata is different again: it belongs to the plugin, and a third-party one will
//! never be in any table here. [`audio`] therefore *looks up* the English term and falls back to
//! it, so an unknown plugin degrades to English instead of showing a placeholder.
//!
//! # Which language
//!
//! [`Language::resolve`] takes the stored preference, where `None` means "follow the system", and
//! [`Language::from_system`] answers for the system: the first of `LC_ALL`, `LC_MESSAGES` and
//! `LANG` that is set, then the operating system itself. Windows sets none of those variables and
//! neither does the Mac application started from Finder, so asking the system is what makes a
//! Japanese install come up in Japanese. A language is stored as its tag (`"ja"`); the name an
//! older build wrote (`"japanese"`) still reads.

#![warn(missing_docs)]

pub mod audio;
pub mod controller;
pub mod messages;

/// What the build script wrote from `locales/`.
mod generated {
    include!(concat!(env!("OUT_DIR"), "/atelier_i18n.rs"));
}

pub use generated::{Key, Language, t};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn locales_map_to_languages_regardless_of_region_or_charset() {
        assert_eq!(
            Language::from_locale("ja_JP.UTF-8"),
            Some(Language::Japanese)
        );
        assert_eq!(Language::from_locale("ja"), Some(Language::Japanese));
        assert_eq!(Language::from_locale("en-GB"), Some(Language::English));
        assert_eq!(Language::from_locale("EN"), Some(Language::English));
        assert_eq!(Language::from_locale("de_DE"), None);
        assert_eq!(Language::from_locale(""), None);
    }

    #[test]
    fn a_stored_preference_outranks_the_environment() {
        assert_eq!(
            Language::resolve(Some(Language::Japanese)),
            Language::Japanese
        );
        // And following the system always lands on a language this build can render.
        assert!(Language::ALL.contains(&Language::resolve(None)));
    }

    #[test]
    fn language_codes_round_trip_and_are_distinct() {
        for language in Language::ALL {
            assert_eq!(Language::from_locale(language.code()), Some(language));
            assert!(!language.endonym().is_empty());
        }
        assert_ne!(Language::English.code(), Language::Japanese.code());
        assert_eq!(Language::default(), Language::English);
    }

    #[test]
    fn a_language_is_stored_as_its_tag_and_the_old_name_still_reads() {
        let json = serde_json::to_string(&Language::Japanese).unwrap();
        assert_eq!(json, "\"ja\"");
        for stored in ["\"ja\"", "\"japanese\""] {
            assert_eq!(
                serde_json::from_str::<Language>(stored).unwrap(),
                Language::Japanese
            );
        }
    }

    #[test]
    fn every_key_has_text_in_every_language() {
        assert!(Key::ALL.len() > 2000, "{} keys", Key::ALL.len());
        for key in Key::ALL {
            for language in Language::ALL {
                let text = key.get(language);
                assert!(!text.trim().is_empty(), "{key:?} is blank in {language:?}");
            }
        }
    }

    #[test]
    fn no_key_is_left_untranslated_by_accident() {
        // Identical text in both languages is legitimate for a few strings — "M" is "M" — but
        // it is nearly always a forgotten translation, so the exceptions are listed rather than
        // waved through.
        // A note value written as a fraction is that fraction in either language, the same way an
        // initial is an initial. The triplet rows are not here, because "triplet" and 「三連」
        // are words rather than numbers and both languages have their own.
        const SHARED: &[Key] = &[
            Key::MuteInitial,
            Key::SoloInitial,
            Key::RecordInitial,
            Key::MonitorInitial,
            Key::SubdivisionEighth,
            Key::SubdivisionSixteenth,
            Key::FormatWav,
            Key::FormatFlac,
            Key::FormatMp3,
            // Dimensional notation is the same in both languages.
            Key::TimbreMapView3d,
            Key::TimbreAxisPc1,
            Key::TimbreAxisPc2,
            Key::TimbreAxisPc3,
            // Product names, which the navigation column shows as names rather than as the
            // sentence 「…設定」 the old tab row needed.
            Key::VoiceSetupVoicevox,
            Key::VoiceSetupDiffSinger,
            Key::VoiceSetupLeapSinger,
            // The event list's column headings, which Japanese list editors print in English too.
            Key::EventListTick,
            Key::EventListStep,
            Key::EventListGate,
        ];
        for key in Key::ALL {
            if SHARED.contains(key) {
                continue;
            }
            assert_ne!(
                key.get(Language::English),
                key.get(Language::Japanese),
                "{key:?} has the same text in both languages"
            );
        }
    }

    #[test]
    fn an_ellipsis_survives_translation() {
        // A menu item that opens a dialog says so with a trailing ellipsis, and a translation
        // that drops it turns "opens a window" into "does it now".
        for key in Key::ALL {
            let english = key.get(Language::English);
            if english.ends_with('…') {
                assert!(
                    key.get(Language::Japanese).ends_with('…'),
                    "{key:?} loses its ellipsis in Japanese"
                );
            }
        }
    }
}
