//! The preset picker at the head of a clip's performance section.
//!
//! One row names the preset the clip's stack currently matches — or says the stack is custom —
//! and opens a menu: the presets for the family the clip's instrument suggests, one entry per
//! other family that opens that family's list, and the two commands that keep the current
//! performance as a preset and delete a kept one. Applying copies the stages into the clip, so
//! every dial below goes on working on them exactly as it did before; see
//! `auris_core::PerformancePreset` for why nothing in the document names the preset.

use auris_i18n::Key;
use auris_session::performance_presets::{ListedPreset, family_hint};
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, Point, prelude::*};

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::prompt::{Prompt, PromptTarget};

/// The name of an instrument family in the interface.
pub fn family_key(family: PresetFamily) -> Key {
    match family {
        PresetFamily::Keys => Key::PresetFamilyKeys,
        PresetFamily::Guitar => Key::PresetFamilyGuitar,
        PresetFamily::Bass => Key::PresetFamilyBass,
        PresetFamily::Drums => Key::PresetFamilyDrums,
        PresetFamily::Strings => Key::PresetFamilyStrings,
        PresetFamily::Brass => Key::PresetFamilyBrass,
        PresetFamily::Woodwinds => Key::PresetFamilyWoodwinds,
        PresetFamily::Synth => Key::PresetFamilySynth,
        PresetFamily::Other => Key::PresetFamilyOther,
    }
}

/// The translated name of a built-in preset, by its stable identifier.
pub fn builtin_key(name: &str) -> Option<Key> {
    Some(match name {
        "piano_ballad" => Key::PresetPianoBallad,
        "piano_pop_comp" => Key::PresetPianoPopComp,
        "jazz_piano_comp" => Key::PresetJazzPianoComp,
        "electric_piano_soul" => Key::PresetElectricPianoSoul,
        "organ_sustain" => Key::PresetOrganSustain,
        "acoustic_strum" => Key::PresetAcousticStrum,
        "rock_rhythm_guitar" => Key::PresetRockRhythmGuitar,
        "funk_cutting_guitar" => Key::PresetFunkCuttingGuitar,
        "lead_guitar" => Key::PresetLeadGuitar,
        "finger_bass" => Key::PresetFingerBass,
        "funk_bass" => Key::PresetFunkBass,
        "synth_bass_tight" => Key::PresetSynthBassTight,
        "pop_drums" => Key::PresetPopDrums,
        "funk_drums" => Key::PresetFunkDrums,
        "jazz_drums" => Key::PresetJazzDrums,
        "machine_drums" => Key::PresetMachineDrums,
        "string_legato" => Key::PresetStringLegato,
        "string_pizzicato" => Key::PresetStringPizzicato,
        "brass_section" => Key::PresetBrassSection,
        "woodwind_solo" => Key::PresetWoodwindSolo,
        "synth_lead" => Key::PresetSynthLead,
        "synth_pad" => Key::PresetSynthPad,
        "synth_arp_tight" => Key::PresetSynthArpTight,
        "chiptune" => Key::PresetChiptune,
        _ => return None,
    })
}

impl AurisApp {
    /// What a preset is called here: a built-in in the interface's language, a kept one as typed.
    fn preset_label(&self, listed: &ListedPreset) -> String {
        match builtin_key(&listed.preset.name).filter(|_| listed.builtin) {
            Some(key) => self.t(key).to_string(),
            None => listed.preset.name.clone(),
        }
    }

    /// The family the clip's instrument suggests.
    fn clip_preset_family(&self, clip: ClipId) -> PresetFamily {
        self.project()
            .midi_clip(clip)
            .and_then(|(track, _)| self.project().track(track))
            .map_or(PresetFamily::Other, family_hint)
    }

    /// The preset the clip's stack is exactly what applying would give, if any.
    fn matching_preset(&self, clip: ClipId, stack: &[NoteTransform]) -> Option<ListedPreset> {
        if stack.is_empty() {
            return None;
        }
        self.session
            .performance_presets()
            .into_iter()
            .find(|listed| {
                self.session
                    .performance_preset_stack(clip, &listed.preset)
                    .is_ok_and(|(candidate, _)| candidate == stack)
            })
    }

    /// The row naming the clip's preset, opening the preset menu.
    pub(crate) fn perform_preset_row(
        &self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut gpui::Context<Self>,
    ) -> AnyElement {
        let value = match self.matching_preset(clip, stack) {
            Some(listed) => self.preset_label(&listed),
            None if stack.is_empty() => self.t(Key::PerformPresetNone).to_string(),
            None => self.t(Key::PerformPresetCustom).to_string(),
        };
        self.picker_row(
            "perform-preset",
            Key::PerformPreset,
            value,
            Self::opens_menu(cx, move |this, at| this.perform_preset_menu(at, clip)),
        )
        .into_any_element()
    }

    /// The presets of one family, each checked when it is what the clip holds.
    fn add_family_presets(
        &self,
        mut menu: ContextMenu,
        clip: ClipId,
        family: PresetFamily,
        presets: &[ListedPreset],
    ) -> ContextMenu {
        let current =
            self.matching_preset(clip, self.session.clip_transforms(clip).unwrap_or_default());
        for listed in presets
            .iter()
            .filter(|listed| listed.preset.family == family)
        {
            menu = menu.toggle(
                self.preset_label(listed),
                MenuCommand::ApplyPerformPreset {
                    clip,
                    name: listed.preset.name.clone(),
                },
                current
                    .as_ref()
                    .is_some_and(|held| held.preset.name == listed.preset.name),
            );
        }
        menu
    }

    /// The preset menu: the suggested family's presets, the other families, save and delete.
    pub(crate) fn perform_preset_menu(&mut self, at: Point<Pixels>, clip: ClipId) -> ContextMenu {
        // Another window or an agent in another process may have kept one since.
        self.session.reload_performance_presets();
        let presets = self.session.performance_presets();
        let suggested = self.clip_preset_family(clip);
        let mut menu = ContextMenu::new(at, self.t(Key::PerformPresetMenu));
        menu = self.add_family_presets(menu, clip, suggested, &presets);
        menu = menu.separator();
        for family in PresetFamily::ALL {
            if family == suggested || !presets.iter().any(|p| p.preset.family == family) {
                continue;
            }
            menu = menu.item(
                format!("{}…", self.t(family_key(family))),
                MenuCommand::ShowPerformPresetFamily { clip, family, at },
            );
        }
        menu = menu.separator().item(
            self.t(Key::PerformPresetSave),
            MenuCommand::ShowPerformPresetSave { clip, at },
        );
        menu.item_if(
            presets.iter().any(|listed| !listed.builtin),
            self.t(Key::PerformPresetDelete),
            MenuCommand::ShowPerformPresetDelete { at },
        )
    }

    /// One family's presets.
    pub(crate) fn perform_preset_family_menu(
        &self,
        at: Point<Pixels>,
        clip: ClipId,
        family: PresetFamily,
    ) -> ContextMenu {
        let presets = self.session.performance_presets();
        let menu = ContextMenu::new(at, self.t(family_key(family)));
        self.add_family_presets(menu, clip, family, &presets)
    }

    /// Which family to keep the current performance under, the suggested one ticked.
    pub(crate) fn perform_preset_save_menu(&self, at: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let suggested = self.clip_preset_family(clip);
        let mut menu = ContextMenu::new(at, self.t(Key::PerformPresetSaveFamily));
        for family in PresetFamily::ALL {
            menu = menu.toggle(
                self.t(family_key(family)),
                MenuCommand::SavePerformPreset { clip, family },
                family == suggested,
            );
        }
        menu
    }

    /// The kept presets, choosing one deletes it.
    pub(crate) fn perform_preset_delete_menu(&self, at: Point<Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(at, self.t(Key::PerformPresetDeleteMenu));
        for listed in self
            .session
            .performance_presets()
            .into_iter()
            .filter(|listed| !listed.builtin)
        {
            menu = menu.item(
                listed.preset.name.clone(),
                MenuCommand::DeletePerformPreset(listed.preset.name),
            );
        }
        menu
    }

    /// Opens the sheet that names a preset kept from the clip's performance.
    pub(crate) fn prompt_for_perform_preset(&mut self, clip: ClipId, family: PresetFamily) {
        let title = self.t(Key::PerformPresetName);
        self.open_prompt(Prompt::new(
            title,
            PromptTarget::SavePerformancePreset { clip, family },
            String::new(),
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{choose, click, paint, with_a_clip};
    use auris_session::prelude::builtin_performance_presets;
    use gpui::TestAppContext;

    #[test]
    fn every_builtin_and_family_has_words_in_the_interface() {
        for preset in builtin_performance_presets() {
            assert!(builtin_key(&preset.name).is_some(), "{}", preset.name);
        }
        for family in PresetFamily::ALL {
            let _ = family_key(family);
        }
    }

    #[test]
    fn builtin_stages_are_in_the_panels_order_so_its_dials_edit_them_in_place() {
        for preset in builtin_performance_presets() {
            let ranks: Vec<_> = preset
                .transforms
                .iter()
                .map(crate::ui::performance::rank)
                .collect();
            assert!(ranks.is_sorted(), "{}: {ranks:?}", preset.name);
        }
    }

    #[gpui::test]
    fn a_preset_is_applied_from_the_panel_and_kept_under_a_new_name(cx: &mut TestAppContext) {
        let (app, cx, _, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.open_clip_in_editor(clip);
        });
        crate::harness::resize(&app, cx, gpui::size(gpui::px(1920.0), gpui::px(3000.0)));
        click("score-performed", cx);
        paint(&app, cx);
        click("perform-preset", cx);
        let family = app.read_with(cx, |app, _| app.clip_preset_family(clip));
        let first = builtin_performance_presets()
            .into_iter()
            .find(|preset| preset.family == family)
            .unwrap();
        choose(
            &app,
            cx,
            &MenuCommand::ApplyPerformPreset {
                clip,
                name: first.name.clone(),
            },
        );
        app.read_with(cx, |app, _| {
            let stack = app.session.clip_transforms(clip).unwrap();
            let expected = app
                .session
                .performance_preset_stack(clip, &first)
                .unwrap()
                .0;
            assert_eq!(stack, expected.as_slice());
            assert_eq!(
                app.matching_preset(clip, stack)
                    .map(|listed| listed.preset.name),
                Some(first.name.clone())
            );
            assert_eq!(app.project().midi_clip(clip).unwrap().1.notes, vec![]);
        });

        app.update(cx, |this, cx| {
            this.menu = None;
            this.run_menu_command(
                MenuCommand::SavePerformPreset {
                    clip,
                    family: PresetFamily::Other,
                },
                cx,
            );
            assert!(matches!(
                this.prompt.as_ref().and_then(|prompt| prompt.target()),
                Some(PromptTarget::SavePerformancePreset { .. })
            ));
        });
        cx.simulate_input("My feel");
        cx.simulate_keystrokes("enter");
        paint(&app, cx);
        app.update(cx, |this, cx| {
            let kept = this.session.performance_preset("My feel").unwrap();
            assert!(!kept.builtin);
            assert_eq!(kept.preset.family, PresetFamily::Other);
            assert_eq!(
                this.matching_preset(clip, this.session.clip_transforms(clip).unwrap())
                    .map(|listed| listed.builtin),
                Some(true),
                "the built-in is still what the clip holds"
            );
            this.run_menu_command(MenuCommand::DeletePerformPreset("My feel".into()), cx);
            assert!(this.session.performance_preset("My feel").is_none());
        });
    }
}
