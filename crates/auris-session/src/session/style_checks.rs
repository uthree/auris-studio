//! Which of the score's checks apply to a song of a given genre and usage.
//!
//! The checks were written against songs with a verse, a pre-chorus and a chorus that lifts,
//! and they are right for those. An ambient piece has no chorus to arrive, a loop has no ending
//! to frame, and a jazz head treats an altered tension as vocabulary rather than a wrong note;
//! held to the song checks, each would be sent back to fix what it meant to do. This table is
//! the one place that says which checks a style keeps. An unset style keeps all of them, so a
//! song written before styles existed is checked as it was.

use auris_core::style::{Genre, Style, Usage};

/// The checks a style keeps.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StyleChecks {
    /// Whether a chorus must lift above the section before it, hold the song's highest note
    /// and leap, and whether a song with no chorus is reported.
    pub chorus_lift: bool,
    /// Bars of intro and outro below which the song is reported, or `None` when the music has
    /// no frame of its own: a loop runs its end into its start, and a cue enters and leaves
    /// with the picture.
    pub frame_bars: Option<f64>,
    /// Whether a chorus must be fuller than the section before it.
    pub chorus_fuller: bool,
    /// Whether a chorus that loops the verse's progression is reported.
    pub chorus_own_progression: bool,
    /// Whether a held note outside the key and its chord is only noted rather than warned
    /// about, because chromatic tensions are the genre's ordinary harmony.
    pub chromatic_harmony: bool,
    /// The integrated loudness, lowest and highest in LUFS, the style's page asks the mix to
    /// land in, or `None` where the page names no level. A loop under a game leaves room for
    /// its sounds and a dance track is loud on purpose; one target for both fits neither.
    pub loudness: Option<(f32, f32)>,
    /// The effect settings the style's page gives a range for, which the linter holds an
    /// inserted effect to. Empty for an unset genre.
    pub effect_ranges: &'static [EffectRange],
}

/// One effect parameter's range as a style's page names it: where a part of this style starts.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EffectRange {
    /// The registry id of the effect, `auris.fx.…`.
    pub effect: &'static str,
    /// The parameter's key.
    pub param: &'static str,
    /// The lowest value the page names, in the parameter's own units.
    pub low: f32,
    /// The highest value the page names.
    pub high: f32,
}

impl StyleChecks {
    /// The level a mix of this style is normalized toward: the middle of its range, or the
    /// standard target where the style names none.
    pub fn loudness_target(&self) -> f32 {
        self.loudness
            .map_or(crate::TARGET_LUFS, |(low, high)| (low + high) / 2.0)
    }
}

/// The checks `style` keeps.
pub fn style_checks(style: Style) -> StyleChecks {
    let genre = style.genre;
    let is = |genres: &[Genre]| genre.is_some_and(|genre| genres.contains(&genre));
    // A song is what the checks were written for; leaving the usage open keeps them.
    let song = matches!(style.usage, None | Some(Usage::Song));
    StyleChecks {
        // A drop, a loop's hook, a jazz head and an orchestral climax are not a chorus lifted
        // by register, and an ambient piece has no arrival at all.
        chorus_lift: song
            && !is(&[
                Genre::Jazz,
                Genre::Orchestral,
                Genre::Ambient,
                Genre::HipHopLofi,
                Genre::DanceElectronic,
            ]),
        frame_bars: song.then_some(super::arrangement_shape::MIN_FRAME_BARS),
        chorus_fuller: !is(&[Genre::Ambient]),
        // Built on one loop by design: a chorus over the verse's progression is the idiom.
        chorus_own_progression: !is(&[Genre::HipHopLofi, Genre::DanceElectronic, Genre::Ambient]),
        chromatic_harmony: is(&[Genre::Jazz, Genre::RnbNeoSoul, Genre::CityPop, Genre::Latin]),
        loudness: loudness(style),
        effect_ranges: genre.map_or(&[], effect_ranges),
    }
}

/// The ranges each page under `docs/genres` writes as "`key` low to high" (the toolbox's tests
/// hold the two to each other). Only settings whose part the page makes plain are here: an
/// amplifier's drive is a guitar's, while an EQ or a compressor goes on many parts, each
/// wanting its own value, and is left to the page. Where a page names several ranges for one
/// key, for a clean, a rhythm and a lead guitar, the entry spans them all.
fn effect_ranges(genre: Genre) -> &'static [EffectRange] {
    const fn range(effect: &'static str, param: &'static str, low: f32, high: f32) -> EffectRange {
        EffectRange {
            effect,
            param,
            low,
            high,
        }
    }
    const AMP: &str = "auris.fx.guitar_amp";
    const REVERB: &str = "auris.fx.reverb";
    match genre {
        Genre::Rock => {
            const RANGES: &[EffectRange] = &[
                range(AMP, "drive_db", 0.0, 34.0),
                range(REVERB, "room_size", 0.3, 0.5),
            ];
            RANGES
        }
        Genre::AnimeVocaloid => {
            const RANGES: &[EffectRange] = &[range(AMP, "drive_db", 24.0, 34.0)];
            RANGES
        }
        Genre::JPop => {
            const RANGES: &[EffectRange] = &[
                range(AMP, "drive_db", 0.0, 22.0),
                range("auris.fx.plate", "decay", 1.5, 2.2),
            ];
            RANGES
        }
        Genre::CityPop => {
            const RANGES: &[EffectRange] = &[
                range(AMP, "drive_db", 0.0, 6.0),
                range("auris.fx.chorus", "mix", 0.3, 0.5),
                range(REVERB, "room_size", 0.4, 0.6),
            ];
            RANGES
        }
        Genre::RnbNeoSoul => {
            const RANGES: &[EffectRange] = &[
                range("auris.fx.tube", "drive_db", 6.0, 12.0),
                range(REVERB, "room_size", 0.6, 0.8),
            ];
            RANGES
        }
        Genre::HipHopLofi => {
            const RANGES: &[EffectRange] = &[
                range("auris.fx.tape", "wow", 0.3, 0.5),
                range("auris.fx.tape", "flutter", 0.3, 0.5),
            ];
            RANGES
        }
        Genre::DanceElectronic => {
            const RANGES: &[EffectRange] = &[
                range("auris.fx.distortion", "drive_db", 6.0, 18.0),
                range("auris.fx.distortion", "mix", 0.3, 0.6),
            ];
            RANGES
        }
        Genre::Jazz => {
            const RANGES: &[EffectRange] = &[
                range(AMP, "drive_db", 0.0, 4.0),
                range(AMP, "treble_db", -6.0, -3.0),
                range(REVERB, "room_size", 0.3, 0.5),
            ];
            RANGES
        }
        Genre::Ambient => {
            const RANGES: &[EffectRange] = &[
                range("auris.fx.chorus", "rate_hz", 0.1, 0.3),
                range(REVERB, "room_size", 0.8, 0.95),
                range("auris.fx.plate", "decay", 5.0, 8.0),
                range("auris.fx.delay", "feedback", 0.5, 0.7),
            ];
            RANGES
        }
        Genre::Orchestral => {
            const RANGES: &[EffectRange] = &[
                range(REVERB, "room_size", 0.8, 0.9),
                range(REVERB, "pre_delay_ms", 20.0, 40.0),
            ];
            RANGES
        }
        Genre::FolkAcoustic => {
            const RANGES: &[EffectRange] = &[range(REVERB, "room_size", 0.2, 0.4)];
            RANGES
        }
        Genre::Latin => {
            const RANGES: &[EffectRange] = &[range(REVERB, "room_size", 0.3, 0.4)];
            RANGES
        }
        Genre::Chiptune => {
            const RANGES: &[EffectRange] = &[range("auris.fx.delay", "mix", 0.15, 0.25)];
            RANGES
        }
    }
}

/// The range each page under `docs/genres` and `docs/usages` names (the toolbox's tests hold
/// the two to each other), the usage first: a loop's
/// headroom for the game's sounds is what decides its level, whatever its genre.
fn loudness(style: Style) -> Option<(f32, f32)> {
    if style.usage == Some(Usage::LoopBgm) {
        return Some((-18.0, -16.0));
    }
    Some(match style.genre? {
        Genre::DanceElectronic => (-11.0, -9.0),
        Genre::Rock => (-12.0, -10.0),
        Genre::AnimeVocaloid => (-14.0, -12.0),
        Genre::Chiptune => (-14.0, -12.0),
        // A page naming one level is held to it within a unit either way.
        Genre::JPop | Genre::CityPop | Genre::RnbNeoSoul => (-15.0, -13.0),
        Genre::HipHopLofi | Genre::FolkAcoustic | Genre::Latin => (-16.0, -14.0),
        Genre::Jazz => (-18.0, -16.0),
        // Kept quiet for the dynamics a build or a drift needs.
        Genre::Orchestral | Genre::Ambient => (-21.0, -18.0),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn of(genre: Option<Genre>, usage: Option<Usage>) -> StyleChecks {
        style_checks(Style { genre, usage })
    }

    #[test]
    fn an_unset_style_keeps_every_check() {
        let checks = of(None, None);
        assert!(checks.chorus_lift && checks.chorus_fuller && checks.chorus_own_progression);
        assert_eq!(checks.frame_bars, Some(8.0));
        assert!(!checks.chromatic_harmony);
        assert_eq!(checks.loudness, None);
        assert_eq!(checks.loudness_target(), crate::TARGET_LUFS);
        let jpop = of(Some(Genre::JPop), Some(Usage::Song));
        assert_eq!(
            StyleChecks {
                loudness: None,
                effect_ranges: &[],
                ..jpop
            },
            checks
        );
    }

    #[test]
    fn a_loop_mixes_quieter_than_its_genre_and_dance_louder() {
        let dance = of(Some(Genre::DanceElectronic), None);
        assert_eq!(dance.loudness, Some((-11.0, -9.0)));
        assert_eq!(dance.loudness_target(), -10.0);
        let looped = of(Some(Genre::DanceElectronic), Some(Usage::LoopBgm));
        assert_eq!(looped.loudness, Some((-18.0, -16.0)));
        assert_eq!(of(None, Some(Usage::LoopBgm)).loudness_target(), -17.0);
        // Jazz keeps its dynamics; only an unset style adds no check.
        assert_eq!(of(Some(Genre::Jazz), None).loudness, Some((-18.0, -16.0)));
        assert_eq!(of(None, Some(Usage::Song)).loudness, None);
    }

    #[test]
    fn a_loop_or_a_cue_has_no_frame_and_no_chorus_to_lift() {
        for usage in [Usage::LoopBgm, Usage::Cue] {
            let checks = of(Some(Genre::JPop), Some(usage));
            assert_eq!(checks.frame_bars, None);
            assert!(!checks.chorus_lift);
        }
    }

    #[test]
    fn genres_without_a_lifted_chorus_are_not_asked_for_one() {
        for genre in [Genre::Jazz, Genre::Ambient, Genre::DanceElectronic] {
            assert!(!of(Some(genre), None).chorus_lift, "{genre:?}");
        }
        assert!(of(Some(Genre::Rock), None).chorus_lift);
    }

    #[test]
    fn only_the_chromatic_genres_relax_the_key() {
        let relaxed: Vec<Genre> = Genre::ALL
            .into_iter()
            .filter(|genre| of(Some(*genre), None).chromatic_harmony)
            .collect();
        assert_eq!(
            relaxed,
            [Genre::CityPop, Genre::RnbNeoSoul, Genre::Jazz, Genre::Latin]
        );
    }
}
