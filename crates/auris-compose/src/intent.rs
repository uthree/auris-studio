//! Plain-language choices resolved into the composer's musical settings.

use serde::{Deserialize, Serialize};

use crate::spec::Mood;
use crate::theory::{key::Key, pitch::PitchClass, scale::ScaleId};

impl crate::SongSpec {
    /// Gives each section its own generated progression and a melody drawn from the seed.
    pub fn use_generated_harmony(&mut self) {
        self.charts.clear();
        self.chart_order.clear();
        self.charts
            .insert("main".into(), crate::theory::chart::Chart::unwritten());
        self.chart_order.push("main".into());
        for (name, section) in &mut self.sections {
            section.chords = name.clone();
            self.charts
                .insert(name.clone(), crate::theory::chart::Chart::unwritten());
            if name != "main" {
                self.chart_order.push(name.clone());
            }
        }
        self.motif.clear();
    }
}

/// Whether to choose a mode from the mood or ask for one explicitly.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Tonality {
    /// Dark moods use minor; the other moods use major.
    #[default]
    Auto,
    /// A major scale.
    Major,
    /// A natural minor scale, with a major dominant available at cadences.
    Minor,
}

impl Tonality {
    /// Resolves the mode while retaining the chosen tonic.
    pub fn key(self, mood: Mood, tonic: PitchClass) -> Key {
        let minor = self == Self::Minor || (self == Self::Auto && mood.brightness < 0.5);
        Key::new(
            tonic,
            if minor {
                ScaleId::Minor
            } else {
                ScaleId::Major
            },
        )
    }
}

/// A tempo choice that does not require knowing BPM.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Pace {
    /// Follow the mood's energy.
    #[default]
    Auto,
    /// A relaxed 80 beats per minute.
    Slow,
    /// A moderate 120 beats per minute.
    Moderate,
    /// A brisk 160 beats per minute.
    Fast,
}

impl Pace {
    /// Resolves the descriptive speed into beats per minute.
    pub fn bpm(self, mood: Mood) -> f64 {
        match self {
            Self::Auto if mood.energy < 0.4 => Self::Slow.bpm(mood),
            Self::Auto if mood.energy > 0.7 => Self::Fast.bpm(mood),
            Self::Auto | Self::Moderate => 120.0,
            Self::Slow => 80.0,
            Self::Fast => 160.0,
        }
    }
}

#[cfg(test)]
mod tests {
    use crate::{SongSpec, frame};

    #[test]
    fn a_dark_minor_song_needs_no_chord_or_tempo_knowledge() {
        let spec = SongSpec::parse("mood = 'dark'\ntonality = 'minor'\npace = 'slow'").unwrap();
        assert!(spec.key.is_minor());
        assert_eq!(spec.tempo, 80.0);
        assert!(spec.charts.values().all(|chart| chart.is_unwritten()));
        let planned = frame::plan(&spec);
        assert!(
            planned
                .sections
                .iter()
                .all(|section| !section.events.is_empty())
        );
        assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
    }

    #[test]
    fn automatic_choices_follow_mood_but_explicit_choices_win() {
        let dark = SongSpec::parse("mood = 'dark'").unwrap();
        let bright = SongSpec::parse("mood = 'bright'").unwrap();
        assert!(dark.key.is_minor());
        assert!(!bright.key.is_minor());
        assert!(dark.tempo < bright.tempo);
        let major = SongSpec::parse("mood = 'dark'\ntonality = 'major'\npace = 'fast'").unwrap();
        assert!(!major.key.is_minor());
        assert_eq!(major.tempo, 160.0);
        let explicit = SongSpec::parse(
            "mood = 'dark'\ntonality = 'minor'\nkey = 'D dorian'\npace = 'slow'\ntempo = 97.5",
        )
        .unwrap();
        assert_eq!(explicit.key.to_text(), "D dorian");
        assert_eq!(explicit.tempo, 97.5);
    }

    #[test]
    fn styles_supply_instruments_with_automatic_or_explicit_harmony() {
        for style in crate::PRESETS {
            let spec = SongSpec::parse(&format!(
                "style = '{}'\nmood = 'dark'\ntonality = 'minor'",
                style.name
            ))
            .unwrap();
            assert_eq!(spec.parts, style.spec().parts);
            assert!(spec.key.is_minor());
            assert!(spec.charts.values().all(|chart| chart.is_unwritten()));
            assert_eq!(SongSpec::parse(&spec.to_toml()).unwrap(), spec);
        }
        let explicit =
            SongSpec::parse("style = 'pop-band'\nchords = '| i | iv | V | i |'").unwrap();
        assert!(
            explicit
                .sections
                .values()
                .all(|section| section.chords == "main")
        );
        for bad in [
            "style = 'unknown'",
            "tonality = 'unknown'",
            "pace = 'unknown'",
        ] {
            assert!(SongSpec::parse(bad).is_err(), "{bad}");
        }
    }
}
