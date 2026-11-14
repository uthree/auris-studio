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
    }
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
        assert_eq!(of(Some(Genre::JPop), Some(Usage::Song)), checks);
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
