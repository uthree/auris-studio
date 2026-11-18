//! The playing techniques a clip's performance can add beyond its four main dials.
//!
//! The performance section draws swing, gate, humanise and shape for every clip. Everything else
//! — a pedal, a strum, ghost notes, bends — is a *technique*: drawn only once the clip uses it, or
//! once somebody has asked for it from the "add a technique" menu, so a clip's panel lists what is
//! shaping it rather than every dial that could. Which techniques the menu offers is
//! [`offers_stage`]'s answer for the clip's instrument.
//!
//! The decisions here are free functions over a stack, for the reason given in
//! [`crate::ui::context_menu`]; the `impl AurisApp` block draws the rows.

use auris_i18n::Key;
use auris_session::live_agent::performance::ResetStage;
use auris_session::prelude::*;
use gpui::{AnyElement, Pixels, Point, prelude::*};

use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::performance::{PerformDial, dial_fraction, offers_dial, offers_stage};
use crate::ui::widgets::{ButtonStyle, button};

/// One technique a clip may use, with the dials and details that belong to it.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Technique {
    /// Releases held on, like a sustain pedal.
    Sustain,
    /// Releases carried to the next change of harmony.
    Pedal,
    /// Written-legato connections played by what they connect.
    Articulate,
    /// Each chord's top voice brought out.
    Voicing,
    /// Ghost notes, and the brush's quiet chords.
    Ghost,
    /// Intermediate notes joining a single-note line.
    Slide,
    /// Damped retriggers in the tail of held notes.
    Mute,
    /// Chords spread across a strumming hand.
    Strum,
    /// Scoops, vibrato, falls, glides and octave doubling.
    Pitch,
}

impl Technique {
    /// Every technique, in the order the panel lists them — the order the stack plays them.
    pub const ALL: [Technique; 9] = [
        Technique::Articulate,
        Technique::Voicing,
        Technique::Ghost,
        Technique::Slide,
        Technique::Mute,
        Technique::Strum,
        Technique::Sustain,
        Technique::Pedal,
        Technique::Pitch,
    ];

    /// What the technique is called in the add menu.
    pub fn label(self) -> Key {
        match self {
            Technique::Sustain => Key::PerformSustain,
            Technique::Pedal => Key::PerformPedal,
            Technique::Articulate => Key::PerformArticulate,
            Technique::Voicing => Key::PerformVoicing,
            Technique::Ghost => Key::PerformTechniqueGhost,
            Technique::Slide => Key::PerformTechniqueSlide,
            Technique::Mute => Key::PerformTechniqueMute,
            Technique::Strum => Key::PerformTechniqueStrum,
            Technique::Pitch => Key::PerformPitchSettings,
        }
    }

    /// The stage families the technique installs.
    pub fn stages(self) -> &'static [ResetStage] {
        match self {
            Technique::Sustain => &[ResetStage::Sustain],
            Technique::Pedal => &[ResetStage::Pedal],
            Technique::Articulate => &[ResetStage::Articulation],
            Technique::Voicing => &[ResetStage::Voicing],
            Technique::Ghost => &[ResetStage::Ghost],
            Technique::Slide => &[ResetStage::Slide],
            Technique::Mute => &[ResetStage::Mute],
            Technique::Strum => &[ResetStage::Strum],
            Technique::Pitch => &[ResetStage::Pitch, ResetStage::Octaves],
        }
    }

    /// The dial adding the technique turns, and how far: a moderate start that is audible and
    /// easy to judge from. Melodic expression has no single dial to start, so it only opens.
    pub fn start(self) -> Option<(PerformDial, f32)> {
        Some(match self {
            // One beat of hold, out of the dial's whole note.
            Technique::Sustain => (PerformDial::Sustain, 0.25),
            Technique::Pedal => (PerformDial::Pedal, 0.5),
            Technique::Articulate => (PerformDial::Articulate, 0.5),
            Technique::Voicing => (PerformDial::Voicing, 0.5),
            Technique::Ghost => (PerformDial::GhostDensity, 0.5),
            Technique::Slide => (PerformDial::Slide, 0.5),
            Technique::Mute => (PerformDial::Mute, 0.5),
            // Twenty milliseconds across the chord: heard as a strum, not as a smear.
            Technique::Strum => (PerformDial::Stroke, 0.2),
            Technique::Pitch => return None,
        })
    }

    /// Whether the clip's stack holds any of the technique's stages.
    pub fn in_use(self, stack: &[NoteTransform]) -> bool {
        stack
            .iter()
            .filter_map(ResetStage::of)
            .any(|stage| self.stages().contains(&stage))
    }

    /// Whether the technique is offered on an instrument of `family` with this stack.
    pub fn offered(self, family: PresetFamily, stack: &[NoteTransform]) -> bool {
        self.stages()
            .iter()
            .any(|stage| offers_stage(family, stack, *stage))
    }
}

/// The techniques the panel draws: the ones in use, and the ones somebody added and may not have
/// turned up yet. An added technique stays drawn when its dial goes back to zero, so a bar being
/// dragged down does not vanish from under the pointer.
pub fn techniques_shown(
    family: PresetFamily,
    stack: &[NoteTransform],
    added: &[Technique],
) -> Vec<Technique> {
    Technique::ALL
        .into_iter()
        .filter(|technique| {
            technique.in_use(stack)
                || (added.contains(technique) && technique.offered(family, stack))
        })
        .collect()
}

/// The techniques the add menu lists: offered on this instrument and not drawn already.
pub fn techniques_to_add(
    family: PresetFamily,
    stack: &[NoteTransform],
    added: &[Technique],
) -> Vec<Technique> {
    let shown = techniques_shown(family, stack, added);
    Technique::ALL
        .into_iter()
        .filter(|technique| technique.offered(family, stack) && !shown.contains(technique))
        .collect()
}

impl AurisApp {
    /// The techniques somebody added to `clip` from the menu in this window.
    pub(crate) fn added_techniques(&self, clip: ClipId) -> Vec<Technique> {
        self.performance_added
            .iter()
            .filter(|(held, _)| *held == clip)
            .map(|(_, technique)| *technique)
            .collect()
    }

    /// Adds a technique to a clip: draws it, and starts its dial where [`Technique::start`] says.
    pub(crate) fn add_perform_technique(&mut self, clip: ClipId, technique: Technique) {
        if !self.performance_added.contains(&(clip, technique)) {
            self.performance_added.push((clip, technique));
        }
        match technique.start() {
            Some((dial, fraction)) => self.set_perform_dial(clip, dial, fraction),
            None => {
                // Melodic expression has many dials and no one of them to start: open them.
                self.performance_details[3] = true;
            }
        }
    }

    /// The rows of the techniques a clip uses, then the button that adds another.
    pub(crate) fn technique_rows(
        &mut self,
        clip: ClipId,
        stack: &[NoteTransform],
        cx: &mut gpui::Context<Self>,
    ) -> Vec<AnyElement> {
        let family = self.clip_preset_family(clip);
        let added = self.added_techniques(clip);
        let mut rows = Vec::new();
        for technique in techniques_shown(family, stack, &added) {
            match technique {
                Technique::Sustain => {
                    rows.push(self.performance_slider(clip, PerformDial::Sustain, stack, cx))
                }
                Technique::Pedal => {
                    rows.push(self.performance_slider(clip, PerformDial::Pedal, stack, cx))
                }
                Technique::Articulate => {
                    rows.push(self.performance_slider(clip, PerformDial::Articulate, stack, cx))
                }
                Technique::Voicing => {
                    rows.push(self.performance_slider(clip, PerformDial::Voicing, stack, cx))
                }
                Technique::Slide => {
                    rows.push(self.performance_slider(clip, PerformDial::Slide, stack, cx))
                }
                Technique::Mute => {
                    rows.push(self.performance_slider(clip, PerformDial::Mute, stack, cx))
                }
                Technique::Ghost => {
                    if offers_dial(family, stack, PerformDial::Brush) {
                        rows.push(self.performance_slider(clip, PerformDial::Brush, stack, cx));
                    }
                    rows.extend(self.ghost_detail_rows(clip, stack, cx));
                }
                Technique::Strum => {
                    rows.push(self.performance_slider(clip, PerformDial::Stroke, stack, cx));
                    if dial_fraction(stack, PerformDial::Stroke) > 0.0 {
                        rows.push(self.stroke_direction_row(clip, stack, cx));
                    }
                    rows.extend(self.strum_detail_rows(clip, stack, cx));
                }
                Technique::Pitch => rows.extend(self.pitch_detail_rows(clip, stack, cx)),
            }
        }
        if !techniques_to_add(family, stack, &added).is_empty() {
            rows.push(
                button(
                    "perform-add-technique",
                    self.t(Key::PerformAddTechnique),
                    ButtonStyle::Normal,
                    false,
                    self.theme.accent,
                    &self.theme,
                    Self::opens_menu(cx, move |this, at| this.add_technique_menu(at, clip)),
                )
                .into_any_element(),
            );
        }
        rows
    }

    /// The techniques that can still be added to `clip`.
    fn add_technique_menu(&self, anchor: Point<Pixels>, clip: ClipId) -> ContextMenu {
        let stack = self.session.clip_transforms(clip).unwrap_or(&[]);
        let family = self.clip_preset_family(clip);
        let mut menu = ContextMenu::new(anchor, self.t(Key::PerformAddTechniqueMenu));
        for technique in techniques_to_add(family, stack, &self.added_techniques(clip)) {
            menu = menu.item(
                self.t(technique.label()),
                MenuCommand::AddPerformTechnique { clip, technique },
            );
        }
        menu
    }
}

#[cfg(test)]
impl AurisApp {
    /// Draws techniques on a clip without starting them, as a test's starting layout.
    pub(crate) fn show_techniques(&mut self, clip: ClipId, techniques: &[Technique]) {
        for technique in techniques {
            if !self.performance_added.contains(&(clip, *technique)) {
                self.performance_added.push((clip, *technique));
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unused_technique_is_listed_to_add_and_a_used_one_is_drawn() {
        let empty: &[NoteTransform] = &[];
        assert!(techniques_shown(PresetFamily::Keys, empty, &[]).is_empty());
        let to_add = techniques_to_add(PresetFamily::Keys, empty, &[]);
        assert!(to_add.contains(&Technique::Pedal));
        assert!(to_add.contains(&Technique::Strum));
        // A keyboard is not offered a damped string or a slide; its melodic expression is only
        // the octave doubling, so that is offered.
        assert!(!to_add.contains(&Technique::Mute));
        assert!(!to_add.contains(&Technique::Slide));
        assert!(to_add.contains(&Technique::Pitch));

        let pedalled = [NoteTransform::Pedal { amount: 0.5 }];
        assert_eq!(
            techniques_shown(PresetFamily::Keys, &pedalled, &[]),
            vec![Technique::Pedal]
        );
        assert!(!techniques_to_add(PresetFamily::Keys, &pedalled, &[]).contains(&Technique::Pedal));
    }

    #[test]
    fn an_added_technique_stays_drawn_at_zero_and_one_in_use_shows_on_any_instrument() {
        let empty: &[NoteTransform] = &[];
        assert_eq!(
            techniques_shown(PresetFamily::Guitar, empty, &[Technique::Mute]),
            vec![Technique::Mute]
        );
        // Added from the menu on another clip, or before the instrument changed: not offered,
        // so not drawn.
        assert!(techniques_shown(PresetFamily::Keys, empty, &[Technique::Mute]).is_empty());
        // A stage a preset or an agent left behind is drawn, so it can be turned off.
        let muted = [NoteTransform::Mute { amount: 0.4 }];
        assert_eq!(
            techniques_shown(PresetFamily::Keys, &muted, &[]),
            vec![Technique::Mute]
        );
    }

    #[test]
    fn starting_a_technique_installs_its_stage() {
        for technique in Technique::ALL {
            let Some((dial, fraction)) = technique.start() else {
                continue;
            };
            let stack = crate::ui::performance::with_dial(&[], dial, fraction, 7);
            assert!(technique.in_use(&stack), "{technique:?} started nothing");
        }
    }

    #[gpui::test]
    fn the_add_menu_starts_a_technique_and_draws_its_dial(cx: &mut gpui::TestAppContext) {
        use crate::harness::{choose, click, paint, with_a_clip};
        let (app, cx, track, clip) = with_a_clip(cx);
        app.update(cx, |this, _| {
            this.panels = crate::dock::PanelLayout::default();
            this.select_track(track);
            this.select_clip(Some(clip));
        });
        paint(&app, cx);
        // Nothing is in use, so no technique has a row yet; the pedal's is dial 46.
        assert!(cx.debug_bounds("perform-dial-46").is_none());
        click("perform-add-technique", cx);
        choose(
            &app,
            cx,
            &MenuCommand::AddPerformTechnique {
                clip,
                technique: Technique::Pedal,
            },
        );
        paint(&app, cx);
        assert!(cx.debug_bounds("perform-dial-46").is_some());
        app.update(cx, |this, _| {
            assert_eq!(
                this.session.clip_transforms(clip).unwrap(),
                &[NoteTransform::Pedal { amount: 0.5 }]
            );
            // Turned back to nothing, the stage goes but the row stays under the pointer.
            this.set_perform_dial(clip, PerformDial::Pedal, 0.0);
            assert!(this.session.clip_transforms(clip).unwrap().is_empty());
        });
        paint(&app, cx);
        assert!(cx.debug_bounds("perform-dial-46").is_some());
    }
}
