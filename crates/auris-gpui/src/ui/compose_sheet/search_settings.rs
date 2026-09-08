//! Pure settings for a bounded search of the current song sheet.

use auris_session::composition_search::{
    DensityTarget, ParameterBounds, PartDensity, SearchMethod, SearchSpace, SectionIntensity,
    SongSearchRequest,
};
use auris_session::prelude::{PartSpec, Role, SongSpec};

/// The controls held separately from the song specification until a winner is applied.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct SearchSettings {
    /// Proposal strategy.
    pub algorithm: SearchMethod,
    /// Maximum attempted compositions, including failures.
    pub attempt_budget: usize,
    /// Desired total written notes per bar across the arrangement.
    pub target_notes_per_bar: f64,
    /// Independent randomness for proposing settings.
    pub search_seed: u64,
    /// One part whose density can move; absent keeps all part densities fixed.
    pub part: Option<String>,
    /// One section whose intensity can move; absent keeps all intensities fixed.
    pub section: Option<String>,
}

impl Default for SearchSettings {
    fn default() -> Self {
        Self::for_song(&SongSpec::default())
    }
}

impl SearchSettings {
    /// Selects a played, effective density dial, preferring the melody.
    pub(crate) fn for_song(song: &SongSpec) -> Self {
        let part = song
            .parts
            .iter()
            .filter(|part| searchable_part(song, part))
            .min_by_key(|part| part.role != Role::Melody)
            .map(|part| part.name.clone());
        Self {
            algorithm: SearchMethod::HillClimb,
            attempt_budget: 24,
            target_notes_per_bar: 24.0,
            search_seed: 42,
            part,
            section: song.form.first().cloned(),
        }
    }

    /// Clears choices no longer offered by the current song and reports whether any changed.
    ///
    /// A removed or ineffective parameter returns to fixed. Valid choices and all other
    /// settings are retained; selecting a replacement remains an explicit choice.
    pub(crate) fn reconcile(&mut self, song: &SongSpec) -> bool {
        let missing_part = self.part.as_ref().is_some_and(|name| {
            !song
                .parts
                .iter()
                .any(|part| &part.name == name && searchable_part(song, part))
        });
        let missing_section = self
            .section
            .as_ref()
            .is_some_and(|name| !song.form.contains(name) || !song.sections.contains_key(name));
        if missing_part {
            self.part = None;
        }
        if missing_section {
            self.section = None;
        }
        missing_part || missing_section
    }

    /// Captures a request without changing the sheet or document.
    ///
    /// Pinning an automatic density uses the writer's current default, so the initial hill
    /// climbing candidate keeps the same effective density as the authored song.
    pub(crate) fn to_request(&self, song: &SongSpec) -> SongSearchRequest {
        let mut base = song.clone();
        if let Some(name) = &self.part
            && let Some(part) = base.parts.iter_mut().find(|part| &part.name == name)
        {
            let automatic = if part.role.is_drum() {
                0.5
            } else {
                base.mood.density()
            };
            part.density.get_or_insert(automatic);
        }
        let bounds = || ParameterBounds {
            min: 0.0,
            max: 1.0,
            step: 0.1,
        };
        SongSearchRequest {
            composition_seed: base.seed,
            base,
            space: SearchSpace {
                part_density: self.part.clone().map(|part| PartDensity {
                    part,
                    bounds: bounds(),
                }),
                section_intensity: self.section.clone().map(|section| SectionIntensity {
                    section,
                    bounds: bounds(),
                }),
            },
            attempt_budget: self.attempt_budget,
            search_seed: self.search_seed,
            algorithm: self.algorithm,
            evaluator: DensityTarget {
                target_notes_per_bar: self.target_notes_per_bar,
            },
        }
    }
}

/// Whether changing this part's density can reach a writer in the played form.
pub(crate) fn searchable_part(song: &SongSpec, part: &PartSpec) -> bool {
    part.rhythm.is_none()
        && !matches!(part.role, Role::Crash | Role::Riser)
        && song.form.iter().any(|name| {
            song.sections.get(name).is_some_and(|section| {
                (section.parts.is_empty() || section.parts.contains(&part.name))
                    && section
                        .tweaks
                        .get(&part.name)
                        .is_none_or(|tweak| tweak.density.is_none() && tweak.rhythm.is_none())
            })
        })
}

#[cfg(test)]
mod tests {
    use super::*;
    use auris_session::prelude::{Mood, preset};

    #[test]
    fn request_pins_the_writer_default_and_retains_both_seeds() {
        let mut song = SongSpec {
            seed: 917,
            mood: Mood::named("calm").unwrap(),
            parts: vec![PartSpec::of_role("lead", Role::Melody)],
            ..SongSpec::default()
        };
        for section in song.sections.values_mut() {
            section.parts.clear();
        }
        let settings = SearchSettings::for_song(&song);
        let request = settings.to_request(&song);
        assert_eq!(request.base.parts[0].density, Some(song.mood.density()));
        assert_eq!(song.parts[0].density, None);
        assert_eq!(request.composition_seed, 917);
        assert_eq!(request.search_seed, 42);
        assert!(request.space.validate(&request.base).is_ok());
    }

    #[test]
    fn density_choices_exclude_parts_shadowed_in_every_played_section() {
        let mut song = SongSpec {
            parts: vec![PartSpec::of_role("lead", Role::Melody)],
            ..SongSpec::default()
        };
        for section in song.sections.values_mut() {
            section.parts.clear();
            section.tweaks.entry("lead".into()).or_default().density = Some(0.8);
        }
        let settings = SearchSettings::for_song(&song);
        assert_eq!(settings.part, None);
        assert!(settings.section.is_some());
        assert!(settings.to_request(&song).space.validate(&song).is_ok());
    }

    #[test]
    fn automatic_drum_density_matches_the_drum_writer() {
        let mut song = SongSpec {
            mood: Mood::named("calm").unwrap(),
            parts: vec![PartSpec::of_role("kick", Role::Kick)],
            ..SongSpec::default()
        };
        for section in song.sections.values_mut() {
            section.parts.clear();
        }
        let request = SearchSettings::for_song(&song).to_request(&song);
        assert_eq!(request.base.parts[0].density, Some(0.5));
    }

    #[test]
    fn renamed_or_ineffective_parts_return_to_fixed() {
        let original = preset("chiptune").unwrap().spec();
        for rename in [true, false] {
            let mut song = original.clone();
            let mut settings = SearchSettings::for_song(&song);
            let selected = settings.part.clone().unwrap();
            if rename {
                song.parts
                    .iter_mut()
                    .find(|part| part.name == selected)
                    .unwrap()
                    .name = "renamed-lead".into();
            } else {
                for section in song.sections.values_mut() {
                    section.tweaks.entry(selected.clone()).or_default().density = Some(0.5);
                }
            }
            let mut expected = settings.clone();
            expected.part = None;
            assert!(settings.reconcile(&song));
            assert_eq!(settings, expected);
            assert!(!settings.reconcile(&song), "reconciliation is idempotent");
        }
    }

    #[test]
    fn switching_presets_preserves_valid_choices_and_numeric_settings() {
        let original = preset("chiptune").unwrap().spec();
        let replacement = preset("ambient").unwrap().spec();
        let mut settings = SearchSettings::for_song(&original);
        settings.algorithm = SearchMethod::Random;
        settings.attempt_budget = 128;
        settings.target_notes_per_bar = 17.0;
        settings.search_seed = 876;
        let mut expected = settings.clone();
        expected.part = None;
        assert!(settings.reconcile(&replacement));
        assert_eq!(settings, expected);
        assert_eq!(settings.section.as_deref(), Some("intro"));
        assert!(settings.to_request(&replacement).validate().is_ok());
    }

    #[test]
    fn unplayed_or_deleted_sections_return_to_fixed() {
        let original = preset("chiptune").unwrap().spec();
        for delete_section in [false, true] {
            let mut song = original.clone();
            let mut settings = SearchSettings::for_song(&song);
            let selected = settings.section.clone().unwrap();
            if delete_section {
                song.sections.remove(&selected);
            } else {
                song.form.retain(|name| name != &selected);
            }
            let mut expected = settings.clone();
            expected.section = None;
            assert!(settings.reconcile(&song));
            assert_eq!(settings, expected);
        }
    }
}
