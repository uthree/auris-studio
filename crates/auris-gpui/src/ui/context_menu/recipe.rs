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
    /// Every preset, aimed at one place on one track.
    pub(crate) fn preset_picker_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        start: Ticks,
    ) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuGenerateClip));
        for preset in ClipPreset::ALL {
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
        for preset in ClipPreset::ALL {
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

    /// Every register a generated clip can be moved to.
    pub(crate) fn clip_octave_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let current = self.session.clip_recipe(clip).map(|recipe| recipe.octave);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PartOctave));
        // Highest first, because that is the way a register reads on a keyboard and on every
        // stave: going down the menu should go down in pitch.
        for octave in crate::ui::part::octave_choices().rev() {
            menu = menu.toggle(
                crate::ui::part::octave_text(octave),
                MenuCommand::SetClipOctave { clip, octave },
                current == Some(octave),
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
        generated_clip_rows(
            menu,
            clip,
            self.session.clip_recipe(clip).is_some(),
            self.t(Key::MenuRerollClip),
            self.t(Key::MenuRegenerateClip),
            self.t(Key::MenuFreezeClip),
        )
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

/// A seed no clip in the project is using, for the next one that needs one.
///
/// Counted up from the highest in use rather than drawn at random, so a session writes the same
/// run of clips twice and a phrase somebody wants back can be reached again.
fn next_seed(project: &Project) -> u64 {
    project
        .tracks
        .iter()
        .filter_map(|track| track.kind.as_instrument())
        .flat_map(|track| &track.clips)
        .filter_map(|clip| clip.recipe.as_ref())
        .map(|recipe| recipe.seed)
        .max()
        .map_or(1, |highest| highest.wrapping_add(1))
}

/// Where a generated clip goes, and how long it is.
///
/// The cycle region when there is one, for the same reason a progression uses it: setting the
/// cycle over the part of the song being worked on and then acting on it is how the rest of the
/// application already behaves. Four bars from the pointer otherwise, which is enough of a phrase
/// to judge and short enough to throw away.
pub(super) fn generation_range(
    loop_region: Option<(Ticks, Ticks)>,
    tick: Ticks,
    signatures: &SignatureMap,
) -> (Ticks, Ticks) {
    match loop_region {
        Some((from, to)) if to > from => (from.max_zero(), to - from),
        _ => {
            // Four *bars*, not four bar lengths: across a meter change those differ, and what
            // "four bars" means is the four the ruler counts.
            let tick = tick.max_zero();
            let first = signatures.bar_of(tick);
            let length = signatures.bar_start(first + 4) - signatures.bar_start(first);
            (tick, length)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::context_menu::{MenuEntry, meters};
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
    }

    #[test]
    fn a_generated_clip_goes_where_the_cycle_is_when_there_is_one() {
        let bar = TimeSignature::new(4, 4).ticks_per_bar();

        // No cycle: four bars from where the pointer was — enough of a phrase to judge, and
        // short enough to throw away.
        assert_eq!(
            generation_range(None, bar * 2, &meters()),
            (bar * 2, bar * 4)
        );

        // A cycle wins, and the clip is exactly as long as it.
        assert_eq!(
            generation_range(Some((bar * 8, bar * 16)), bar, &meters()),
            (bar * 8, bar * 8)
        );

        // An empty cycle is not a range, so the pointer decides again.
        assert_eq!(
            generation_range(Some((bar * 4, bar * 4)), Ticks::ZERO, &meters()),
            (Ticks::ZERO, bar * 4)
        );
    }
}
