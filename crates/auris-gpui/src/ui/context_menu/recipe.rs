//! The dials of a clip the composer wrote: which preset, which groove, which register, and what
//! rewriting one says afterwards.
//!
//! Barely context-menu code. A [`ClipRecipe`] is a document idea, and everything here is about
//! setting one dial on one and reporting what came out — the menus that offer the choices are
//! only the way the question gets asked, and the same dials are asked for by the part panel's
//! buttons. It is its own file because it is the one family in this module that would still make
//! sense if every menu in the application were replaced by something else. Freezing a whole track
//! is here rather than with the other track work for that reason: what it acts on is the recipes.

use auris_i18n::{Key, messages};
use auris_session::prelude::*;

use gpui::{Pixels, Point};

use crate::app::AurisApp;

use super::{ContextMenu, MenuCommand};

impl AurisApp {
    /// Rewrites one clip from its current recipe and the harmony underneath it.
    pub(crate) fn regenerate_clip(&mut self, clip: ClipId) {
        match self.session.regenerate_clip(clip) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuRegenerateClip, &error)),
        }
    }

    /// The presets appropriate to this track, aimed at one place on its timeline.
    pub(crate) fn preset_picker_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        start: Ticks,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuGenerateClip));
        let Some(kind) = self.project().track(track).map(|track| &track.kind) else {
            return menu;
        };
        for preset in presets_for_track(kind) {
            menu = menu.item(
                self.t(preset_key(preset)),
                MenuCommand::GenerateClip {
                    track,
                    start,
                    preset,
                },
            );
        }
        menu
    }

    /// Every preset, aimed at a clip that already has one.
    ///
    /// Ticks the one it is now: this menu is opened from a button showing that same name, and a
    /// list of six with nothing marked would leave the reader checking the button behind it.
    pub(crate) fn clip_preset_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self.session.clip_recipe(clip).map(|recipe| recipe.preset);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartPreset));
        let Some(kind) = self
            .project()
            .midi_clip(clip)
            .and_then(|(track, _)| self.project().track(track))
            .map(|track| &track.kind)
        else {
            return menu;
        };
        for preset in presets_for_track(kind) {
            menu = menu.toggle(
                self.t(preset_key(preset)),
                MenuCommand::SetClipPreset { clip, preset },
                current == Some(preset),
            );
        }
        menu
    }

    /// Every way of dividing a beat, aimed at one generated clip.
    pub(crate) fn clip_subdivision_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self
            .session
            .clip_recipe(clip)
            .map(|recipe| recipe.subdivision);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartSubdivision));
        for subdivision in Subdivision::ALL {
            menu = menu.toggle(
                self.t(subdivision_key(subdivision)),
                MenuCommand::SetClipSubdivision { clip, subdivision },
                current == Some(subdivision),
            );
        }
        menu
    }

    /// Every groove the composer knows by name, aimed at one drum clip.
    pub(crate) fn clip_groove_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self
            .session
            .clip_recipe(clip)
            .map(|recipe| recipe.groove.clone());
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartGroove));
        for groove in groove_catalog() {
            menu = menu.toggle(
                // The hyphenated identifier is what a specification writes; a menu row is a
                // place for what the groove sounds like.
                auris_i18n::audio::theory_description(groove.description, self.language()),
                MenuCommand::SetClipGroove {
                    clip,
                    groove: groove.name,
                },
                current.as_deref() == Some(groove.name),
            );
        }
        menu
    }

    /// Writes another take of a generated clip, and says what came out.
    pub(crate) fn reroll_clip(&mut self, clip: ClipId) {
        match self.session.reroll_clip(clip) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuRerollClip, &error)),
        }
    }

    /// Keeps a generated clip's notes and forgets how they got there.
    pub(crate) fn freeze_clip(&mut self, clip: ClipId) {
        match self.session.freeze_clip(clip) {
            Ok(()) => self.set_status(self.t(Key::ClipKept)),
            Err(error) => self.set_failed_status(self.failure(Key::MenuFreezeClip, &error)),
        }
    }

    /// Stops every clip on a track from being written again.
    ///
    /// The count is reported rather than a bare confirmation, because this acts on clips the user
    /// is not necessarily looking at — a track scrolled past the bottom of the panel has clips on
    /// it too, and "kept 6" is the difference between believing that and checking.
    pub(crate) fn freeze_track(&mut self, track: TrackId) {
        match self.session.freeze_track(track) {
            Ok(count) => {
                let language = self.language();
                self.set_status(messages::track_kept(language, count));
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuFreezeTrack, &error)),
        }
    }

    /// Makes a generated clip a different kind of part, keeping its seed and its dials.
    ///
    /// The seed is deliberately kept. A recipe's dials mean the same thing whichever part reads
    /// them, so trying the same idea as a bass line and then as an arpeggio is one click either
    /// way rather than a click and then four dials set again from memory.
    pub(crate) fn set_clip_preset(&mut self, clip: ClipId, preset: ClipPreset) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.preset == preset {
            return;
        }
        let recipe = crate::ui::part::with_preset(recipe, preset);
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip(preset, clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip from a seed somebody typed.
    pub(crate) fn set_clip_seed(&mut self, clip: ClipId, seed: u64) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.seed == seed {
            return;
        }
        let recipe = recipe.with_seed(seed);
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Gives a generated clip a different groove.
    pub(crate) fn set_clip_groove(&mut self, clip: ClipId, groove: &str) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.groove == groove {
            return;
        }
        let recipe = ClipRecipe {
            groove: groove.to_string(),
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip over a beat divided a different way.
    pub(crate) fn set_clip_subdivision(&mut self, clip: ClipId, subdivision: Subdivision) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.subdivision == subdivision {
            return;
        }
        let recipe = ClipRecipe {
            subdivision,
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// Writes a generated clip in a different register.
    pub(crate) fn set_clip_octave(&mut self, clip: ClipId, octave: i32) {
        let Some(recipe) = self.session.clip_recipe(clip) else {
            return;
        };
        if recipe.octave == octave {
            return;
        }
        let recipe = ClipRecipe {
            octave,
            ..recipe.clone()
        };
        match self.session.set_clip_recipe(clip, recipe) {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(Key::MenuGenerateClip, &error)),
        }
    }

    /// The rows a generated clip adds to its own menu.
    pub(super) fn generated_clip_rows(&self, menu: ContextMenu, clip: ClipId) -> ContextMenu {
        let anchor = menu.anchor;
        let menu = generated_clip_rows(
            menu,
            clip,
            self.session.clip_recipe(clip).is_some(),
            self.t(Key::MenuRerollClip),
            self.t(Key::MenuRegenerateClip),
            self.t(Key::MenuFreezeClip),
        );
        let has_voices = self.session.clip_recipe(clip).is_some_and(|recipe| {
            recipe.drum_voices.iter().any(|voice| {
                voice.recipe.is_some()
                    && recipe
                        .drum_map
                        .as_ref()
                        .is_none_or(|map| map.voices.contains_key(&voice.role))
            })
        });
        menu.item_if(
            has_voices,
            self.t(Key::PresetDrums),
            MenuCommand::DrumVoicesMenu { clip, anchor },
        )
    }

    /// The independent rhythmic writers inside a kit, using the clip's stored names.
    pub(crate) fn drum_voices_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        drum_voice_rows(
            ContextMenu::new(anchor, self.t(Key::PresetDrums)),
            clip,
            self.session.clip_recipe(clip),
            self.t(Key::MenuRegenerateClip),
            self.t(Key::MenuRerollClip),
        )
    }

    /// Rewrites one kit voice and refreshes note selection after the stored indices change.
    pub(crate) fn rewrite_drum_voice(&mut self, clip: ClipId, voice: &str, reroll: bool) {
        let result = if reroll {
            self.session.reroll_drum_voice(clip, voice)
        } else {
            self.session.regenerate_drum_voice(clip, voice)
        };
        match result {
            Ok(_) => {
                self.forget_rewritten_notes(clip);
                self.report_clip_preset(clip);
            }
            Err(error) => self.set_failed_status(self.failure(
                if reroll {
                    Key::MenuRerollClip
                } else {
                    Key::MenuRegenerateClip
                },
                &error,
            )),
        }
    }

    /// A seed nobody has used yet, for the next clip that needs one.
    pub(super) fn next_seed(&self) -> u64 {
        next_seed(self.project())
    }

    /// Says what was written, in the words the preset picker used.
    pub(super) fn report_clip(&mut self, preset: ClipPreset, clip: ClipId) {
        let notes = self
            .session
            .project()
            .midi_clip(clip)
            .map_or(0, |(_, midi)| midi.notes.len());
        let name = self.t(preset_key(preset)).to_string();
        self.set_status(messages::clip_written(self.language(), &name, notes));
    }

    /// The same, for a clip that already knows which preset it is.
    pub(super) fn report_clip_preset(&mut self, clip: ClipId) {
        let Some(preset) = self.session.clip_recipe(clip).map(|recipe| recipe.preset) else {
            return;
        };
        self.report_clip(preset, clip);
    }
}

/// Drum writers belong to drum tracks; pitched writers belong to other MIDI tracks.
fn presets_for_track(kind: &TrackKind) -> impl Iterator<Item = ClipPreset> + '_ {
    ClipPreset::ALL
        .into_iter()
        .filter(|preset| kind.holds_notes() && preset.is_drums() == kind.is_drum())
}

/// The name a preset goes by on screen.
pub(crate) fn preset_key(preset: ClipPreset) -> Key {
    match preset {
        ClipPreset::Lead => Key::PresetLead,
        ClipPreset::Chords => Key::PresetChords,
        ClipPreset::Pad => Key::PresetPad,
        ClipPreset::Arp => Key::PresetArp,
        ClipPreset::Bass => Key::PresetBass,
        ClipPreset::Stab => Key::PresetStab,
        ClipPreset::Drums => Key::PresetDrums,
        ClipPreset::Kick => Key::PresetKick,
        ClipPreset::Snare => Key::PresetSnare,
        ClipPreset::Hat => Key::PresetHat,
    }
}

/// The note value a subdivision goes by on screen.
pub(crate) fn subdivision_key(subdivision: Subdivision) -> Key {
    match subdivision {
        Subdivision::Eighth => Key::SubdivisionEighth,
        Subdivision::Sixteenth => Key::SubdivisionSixteenth,
        Subdivision::EighthTriplet => Key::SubdivisionEighthTriplet,
        Subdivision::SixteenthTriplet => Key::SubdivisionSixteenthTriplet,
    }
}

/// Adds the rows that only mean something on a clip the composer wrote.
///
/// Nothing is added to a clip somebody played: every one of these commands would refuse it, and a
/// row that can only say no is worse than no row at all.
///
/// A free function taking its own labels, rather than a method reaching into the application, so
/// that what it decides can be checked without a window — which is the whole reason a menu is
/// plain data here.
fn generated_clip_rows(
    menu: ContextMenu,
    clip: ClipId,
    generated: bool,
    reroll: &str,
    regenerate: &str,
    freeze: &str,
) -> ContextMenu {
    if !generated {
        return menu;
    }
    menu.separator()
        .item(reroll.to_string(), MenuCommand::RerollClip(clip))
        .item(regenerate.to_string(), MenuCommand::RegenerateClip(clip))
        .item(freeze.to_string(), MenuCommand::FreezeClip(clip))
}

/// Fixed accents and unassigned voices have no generic writer to offer in a menu.
fn drum_voice_rows(
    mut menu: ContextMenu,
    clip: ClipId,
    recipe: Option<&ClipRecipe>,
    regenerate: &str,
    reroll: &str,
) -> ContextMenu {
    if let Some(recipe) = recipe {
        for voice in &recipe.drum_voices {
            if voice.recipe.is_none()
                || recipe
                    .drum_map
                    .as_ref()
                    .is_some_and(|map| !map.voices.contains_key(&voice.role))
            {
                continue;
            }
            menu = menu
                .item(
                    format!("{} · {regenerate}", voice.name),
                    MenuCommand::RegenerateDrumVoice {
                        clip,
                        voice: voice.name.clone(),
                    },
                )
                .item(
                    format!("{} · {reroll}", voice.name),
                    MenuCommand::RerollDrumVoice {
                        clip,
                        voice: voice.name.clone(),
                    },
                );
        }
    }
    menu
}

/// A seed no clip in the project is using, for the next one that needs one.
///
/// Counted up from the highest in use rather than drawn at random, so a session writes the same
/// run of clips twice and a phrase somebody wants back can be reached again.
fn next_seed(project: &Project) -> u64 {
    project
        .tracks
        .iter()
        // Every note clip, not only the instrument tracks': a generated clip dragged onto a
        // singer track keeps its recipe — clips move freely between the two kinds — and a seed
        // that walked out of the survey came back as somebody else's "new" phrase.
        .filter_map(|track| track.kind.note_clips())
        .flatten()
        .filter_map(|clip| clip.recipe.as_ref())
        .map(|recipe| recipe.seed)
        .max()
        .map_or(1, |highest| highest.wrapping_add(1))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::context_menu::MenuEntry;
    use gpui::{point, px};

    /// The commands a menu offers, ignoring its labels and its separators.
    fn commands(menu: &ContextMenu) -> Vec<MenuCommand> {
        menu.entries
            .iter()
            .filter_map(|entry| match entry {
                MenuEntry::Item(item) => Some(item.command.clone()),
                MenuEntry::Separator => None,
            })
            .collect()
    }

    #[test]
    fn preset_choices_respect_the_track_kind_even_with_the_same_instrument() {
        let mut project = Project::new("Types", 48_000.0);
        let melodic = project.add_instrument_track("Melodic", "auris.synth.drumkit");
        let drum = project.add_drum_track("Drums", "auris.synth.drumkit");
        let melodic: Vec<_> = presets_for_track(&project.track(melodic).unwrap().kind).collect();
        let drum: Vec<_> = presets_for_track(&project.track(drum).unwrap().kind).collect();
        assert_eq!(melodic.len(), 6);
        assert!(melodic.iter().all(|preset| !preset.is_drums()));
        assert_eq!(
            drum,
            vec![
                ClipPreset::Drums,
                ClipPreset::Kick,
                ClipPreset::Snare,
                ClipPreset::Hat
            ]
        );
        assert!(presets_for_track(&TrackKind::Bus).next().is_none());
    }

    #[test]
    fn a_voice_menu_offers_only_assigned_rhythmic_writers() {
        let piece = compose(&SongSpec::default());
        let kit = piece
            .tracks
            .iter()
            .find(|track| !track.drum_parts.is_empty())
            .unwrap();
        let mut recipe = kit.clips[0].recipe.clone().unwrap();
        let mut accent = recipe.drum_voices[0].clone();
        accent.name = "fixed-accent".into();
        accent.recipe = None;
        recipe.drum_voices.push(accent);
        recipe.drum_map = Some(DrumMap {
            voices: [(DrumRole::Snare, 73)].into_iter().collect(),
        });
        let clip = ClipId(7);
        let menu = drum_voice_rows(
            ContextMenu::new(point(px(0.0), px(0.0)), "Drums"),
            clip,
            Some(&recipe),
            "rewrite",
            "again",
        );
        assert_eq!(
            commands(&menu),
            vec![
                MenuCommand::RegenerateDrumVoice {
                    clip,
                    voice: "snare".into()
                },
                MenuCommand::RerollDrumVoice {
                    clip,
                    voice: "snare".into()
                },
            ]
        );
    }

    #[test]
    fn only_a_clip_the_composer_wrote_is_offered_another_take() {
        let clip = ClipId(7);
        let base = || ContextMenu::new(point(px(0.0), px(0.0)), "Clip");

        let played = generated_clip_rows(base(), clip, false, "again", "rewrite", "keep");
        assert!(
            commands(&played).is_empty(),
            "a clip somebody played was offered a command that would refuse it"
        );

        let written = generated_clip_rows(base(), clip, true, "again", "rewrite", "keep");
        assert_eq!(
            commands(&written),
            vec![
                MenuCommand::RerollClip(clip),
                MenuCommand::RegenerateClip(clip),
                MenuCommand::FreezeClip(clip),
            ]
        );
    }

    #[test]
    fn a_new_clip_takes_a_seed_no_other_clip_is_using() {
        let mut project = Project::new("Song", 48_000.0);
        assert_eq!(
            next_seed(&project),
            1,
            "the first clip has to start somewhere"
        );

        let track = project.add_instrument_track("Keys", "auris.synth.chiptune");
        let clip = project
            .add_midi_clip(track, "One", Ticks::ZERO, Ticks(3840))
            .unwrap();
        if let Some(midi) = project.midi_clip_mut(clip) {
            midi.recipe = Some(ClipRecipe::new(ClipPreset::Lead, 41));
        }
        assert_eq!(next_seed(&project), 42);

        // A clip somebody played holds no seed and must not be counted as holding zero.
        let played = project
            .add_midi_clip(track, "Two", Ticks(3840), Ticks(3840))
            .unwrap();
        assert!(project.midi_clip(played).unwrap().1.recipe.is_none());
        assert_eq!(next_seed(&project), 42);

        // A generated clip dragged onto a singer track keeps its recipe, and its seed stays
        // in the survey: counted only on instrument tracks, seed 41 would be handed out again
        // and the "new" phrase would be the moved one, note for note.
        let singer = project.add_singer_track("Voice", "auris.synth.vocal");
        assert!(project.move_clip_to_track(clip, singer));
        assert_eq!(next_seed(&project), 42, "the moved seed still counts");
    }

    #[gpui::test]
    fn a_right_click_near_a_section_end_generates_only_the_gap_under_the_pointer(
        cx: &mut gpui::TestAppContext,
    ) {
        use crate::harness::{choose, lane_point, open, paint, right_press};
        let (app, cx) = open(cx);
        let bar = TimeSignature::default().ticks_per_bar();
        let track = app.update(cx, |this, _| {
            let track = this.session.add_default_drum_track("Kit").unwrap();
            this.session.set_loop_region(Ticks::ZERO, bar * 64);
            this.session.set_loop_enabled(false);
            this.session.set_section(Ticks::ZERO, Some("Intro".into()));
            this.session.set_section(bar, Some("Verse".into()));
            this.session.set_section(bar * 2, Some("Chorus".into()));
            this.session.set_section(bar * 3, None);
            this.session
                .add_midi_clip(track, "Before", Ticks::ZERO, bar)
                .unwrap();
            this.session
                .add_midi_clip(track, "After", bar * 2, bar)
                .unwrap();
            track
        });
        paint(&app, cx);
        let at = lane_point(&app, cx, track, bar * 2 - Ticks(40));
        right_press(cx, at);
        paint(&app, cx);
        let (command, start) = app.read_with(cx, |this, _| {
            commands(this.menu.as_ref().expect("right-click opens the lane menu"))
                .into_iter()
                .find_map(|command| match command {
                    MenuCommand::ShowPresetPicker { start, .. } => {
                        assert!(start < bar * 2);
                        assert_eq!(this.snap(start), bar * 2);
                        Some((command, start))
                    }
                    _ => None,
                })
                .expect("the lane offers generation")
        });
        choose(&app, cx, &command);
        paint(&app, cx);
        choose(
            &app,
            cx,
            &MenuCommand::GenerateClip {
                track,
                start,
                preset: ClipPreset::Drums,
            },
        );
        app.read_with(cx, |this, _| {
            let clip = this.selected_midi_clip().expect("the new clip is selected");
            assert_eq!((clip.start, clip.end()), (bar, bar * 2));
            assert!(!clip.notes.is_empty());
            assert_eq!(
                this.project()
                    .track(track)
                    .unwrap()
                    .kind
                    .note_clips()
                    .unwrap()
                    .len(),
                3
            );
        });
    }

    #[gpui::test]
    fn the_drum_preset_menu_generates_notes_in_a_new_project(cx: &mut gpui::TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let track = app.update(cx, |this, _| {
            this.session.add_default_drum_track("Kit").unwrap()
        });
        for (index, preset) in ClipPreset::ALL
            .into_iter()
            .filter(|preset| preset.is_drums())
            .enumerate()
        {
            let command = MenuCommand::GenerateClip {
                track,
                start: TimeSignature::default().ticks_per_bar() * (4 * index as i64),
                preset,
            };
            app.update(cx, |this, _| {
                assert!(this.project().harmony.is_empty());
                let menu = this.preset_picker_menu(
                    point(px(200.0), px(200.0)),
                    track,
                    TimeSignature::default().ticks_per_bar() * (4 * index as i64),
                );
                this.open_menu(menu);
            });
            crate::harness::paint(&app, cx);
            crate::harness::choose(&app, cx, &command);
            app.read_with(cx, |this, _| {
                let clip = this
                    .selected_midi_clip()
                    .expect("generation selects its new clip");
                assert!(
                    !clip.notes.is_empty(),
                    "{preset:?} must produce audible hits"
                );
                assert_eq!(clip.length, TimeSignature::default().ticks_per_bar() * 4);
                assert_eq!(clip.recipe.as_ref().unwrap().preset, preset);
                assert!(this.project().harmony.is_empty());
            });
        }
    }
}
