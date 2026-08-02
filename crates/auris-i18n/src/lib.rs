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
//! Fixed strings are a [`Key`] enum built by a macro that takes every language at once:
//!
//! ```
//! # use auris_i18n::{Key, Language};
//! assert_eq!(Key::Mixer.get(Language::English), "Mixer");
//! assert_eq!(Key::Mixer.get(Language::Japanese), "ミキサー");
//! ```
//!
//! Adding a key means writing both translations on adjacent lines, and the `match` the macro
//! generates is exhaustive over `(key, language)` — so a language that is missing a string is a
//! compile error rather than a hole discovered by a user. Strings that interpolate values are
//! functions in [`messages`] instead, which keeps the format string a literal and its
//! placeholders checked by the compiler.
//!
//! Plugin metadata is different again: it belongs to the plugin, and a third-party one will
//! never be in any table here. [`audio`] therefore *looks up* the English term and falls back to
//! it, so an unknown plugin degrades to English instead of showing a placeholder.

#![warn(missing_docs)]

pub mod audio;
pub mod messages;
mod strings;

pub use strings::Key;

use serde::{Deserialize, Serialize};

/// A language the interface is available in.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Language {
    /// English.
    #[default]
    English,
    /// Japanese.
    Japanese,
}

impl Language {
    /// Every language, in the order a picker should list them.
    pub const ALL: [Language; 2] = [Language::English, Language::Japanese];

    /// ISO 639-1 code, as used in settings files and `LANG`.
    pub fn code(self) -> &'static str {
        match self {
            Language::English => "en",
            Language::Japanese => "ja",
        }
    }

    /// The language's name in itself, which is what a language picker must show.
    ///
    /// A picker that lists languages in the *current* language is useless to the person who
    /// cannot read the current one — which is the only person using it.
    pub fn endonym(self) -> &'static str {
        match self {
            Language::English => "English",
            Language::Japanese => "日本語",
        }
    }

    /// The language for a locale string such as `ja_JP.UTF-8` or `en-GB`.
    ///
    /// Only the primary subtag is considered: a regional variant of a language we have is still
    /// that language, and one we do not have is not improved by looking at its region.
    pub fn from_locale(locale: &str) -> Option<Language> {
        let primary = locale
            .split(['_', '-', '.'])
            .next()
            .unwrap_or_default()
            .to_ascii_lowercase();
        Language::ALL.into_iter().find(|l| l.code() == primary)
    }

    /// What the environment asks for, falling back to English.
    ///
    /// `LC_ALL` outranks `LC_MESSAGES`, which outranks `LANG`, which is the order POSIX gives
    /// them; macOS sets `LANG` from the system language for terminal programs. The variables
    /// come first even where the system can be asked directly, because someone who exports
    /// `LANG` for one program means it for that program.
    ///
    /// [`Language::from_system_locale`] is the fallback, and on Windows it is the only thing
    /// that ever answers.
    pub fn from_environment() -> Language {
        ["LC_ALL", "LC_MESSAGES", "LANG"]
            .into_iter()
            .filter_map(|name| std::env::var(name).ok())
            .find_map(|value| Language::from_locale(&value))
            .or_else(Language::from_system_locale)
            .unwrap_or_default()
    }

    /// The language the operating system itself is set to, where there is a way to ask.
    ///
    /// `None` where there is not, which is every platform that answers through the locale
    /// variables [`Language::from_environment`] has already read.
    ///
    /// Windows is why this exists. It sets none of those variables, so a program that reads
    /// only them shows English to every Japanese user — who then has to find the language
    /// setting by reading English.
    pub fn from_system_locale() -> Option<Language> {
        #[cfg(target_os = "windows")]
        {
            use windows_sys::Win32::Globalization::GetUserDefaultLocaleName;

            // `LOCALE_NAME_MAX_LENGTH`, fixed at 85 since Vista. Named here rather than pulled
            // from `Win32_System_SystemServices`, which is an enormous module to import a
            // number from.
            let mut name = [0u16; 85];
            // SAFETY: the pointer is to `name`, which is writable for the length passed as the
            // second argument. The call writes UTF-16 within that bound and nothing else.
            let written = unsafe { GetUserDefaultLocaleName(name.as_mut_ptr(), name.len() as i32) };
            // The returned count includes the terminating null; 0 means the call failed.
            let end = usize::try_from(written).ok()?.checked_sub(1)?;
            Language::from_locale(&String::from_utf16_lossy(name.get(..end)?))
        }
        #[cfg(not(target_os = "windows"))]
        {
            None
        }
    }

    /// Resolves a stored preference, where `None` means "follow the system".
    pub fn resolve(preference: Option<Language>) -> Language {
        preference.unwrap_or_else(Language::from_environment)
    }
}

/// The text for `key` in `language`.
///
/// A free function as well as a method because most call sites read better with the language
/// last: `t(Key::Mixer, self.language)`.
pub fn t(key: Key, language: Language) -> &'static str {
    key.get(language)
}

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
    }

    #[test]
    fn asking_the_system_for_its_locale_answers_or_declines_but_never_panics() {
        // The Windows arm reaches into Win32 and back through UTF-16; the rest decline. Either
        // way `from_environment` has to be able to call it on a machine set to any language.
        let answer = Language::from_system_locale();
        assert!(
            cfg!(target_os = "windows") || answer.is_none(),
            "only Windows has a system locale to read here"
        );
        // And whatever came back has to be a language we can actually render.
        if let Some(language) = answer {
            assert!(Language::ALL.contains(&language));
        }
        // Never panics, and the environment still resolves to something.
        let _ = Language::from_environment();
    }

    #[test]
    fn language_codes_round_trip_and_are_distinct() {
        for language in Language::ALL {
            assert_eq!(Language::from_locale(language.code()), Some(language));
            assert!(!language.endonym().is_empty());
        }
        assert_ne!(Language::English.code(), Language::Japanese.code());
    }

    #[test]
    fn a_language_serialises_as_its_own_name() {
        let json = serde_json::to_string(&Language::Japanese).unwrap();
        assert_eq!(json, "\"japanese\"");
        assert_eq!(
            serde_json::from_str::<Language>(&json).unwrap(),
            Language::Japanese
        );
    }
}
