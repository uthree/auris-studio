//! Menus for a track and for the strip it owns: its own row, its lane, its chain and its routing.
//!
//! One family because they all act on a track rather than on anything in it — the header, the
//! mixer strip, the effect chain, the output and the sends are the same object seen from four
//! panels, and a row offered in one of them is usually offered in the others too. The list of
//! positions a discrete plugin parameter can take is here for the same reason: the plugin it
//! belongs to is a slot in one of these chains.

use auris_i18n::Key;
use auris_session::prelude::*;

use gpui::{Pixels, Point, SharedString};

use crate::app::AurisApp;
use crate::ui::automation::automation_offer;
use crate::ui::transport_bar::input_label;

use super::{ContextMenu, MenuCommand};

impl AurisApp {
    /// The menu for a track, shown by its header and its mixer strip.
    pub(crate) fn track_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let Some(entry) = self.project().track(track) else {
            return self.arrangement_menu(anchor);
        };
        let showing = self.automation_lanes.get(&track).copied();
        // Every clip on this track that is still written from a recipe. Freezing is only offered
        // when there is something to freeze, and the count is what the status line reports back.
        let generated = entry
            .kind
            .as_instrument()
            .map(|inner| {
                inner
                    .clips
                    .iter()
                    .filter(|clip| clip.is_generated())
                    .count()
            })
            .unwrap_or(0);
        let current_color = entry.color;
        // Only an audio track has an input to be recorded from; an instrument track's comes from
        // whatever is playing it.
        let records = entry.kind.as_audio().is_some();
        let menu = ContextMenu::new(anchor, entry.name.clone())
            .item(
                self.t(Key::MenuDuplicateTrack),
                MenuCommand::DuplicateTrack(track),
            )
            .item(self.t(Key::MenuRename), MenuCommand::RenameTrack(track))
            .item(self.t(Key::CmdDeleteTrack), MenuCommand::DeleteTrack(track))
            .separator()
            .toggle(
                self.t(Key::Mute),
                MenuCommand::ToggleTrackMute(track),
                entry.mixer.mute,
            )
            .toggle(
                self.t(Key::Solo),
                MenuCommand::ToggleTrackSolo(track),
                entry.mixer.solo,
            )
            .item(
                self.t(Key::MenuAddEffect),
                MenuCommand::ShowEffectPicker {
                    track: Some(track),
                    at: anchor,
                },
            );
        // Where a take would come from, which is also the only way to arm a track on anything
        // other than the channels the session picked for it.
        let menu = match records {
            true => menu.item(
                self.t(Key::MenuRecordInput),
                MenuCommand::ShowInputPicker { track, at: anchor },
            ),
            false => menu,
        };
        let menu = menu
            .separator()
            .item(
                self.t(Key::MenuRouteTo),
                MenuCommand::ShowOutputPicker { track, at: anchor },
            )
            // Only where there is a bus at all: with none in the project the item can do nothing
            // but open an empty list. One that exists and cannot be sent to *is* worth opening,
            // because the greyed row is the answer to why.
            .item_if(
                self.session.buses().next().is_some(),
                self.t(Key::MenuAddSend),
                MenuCommand::ShowSendPicker { track, at: anchor },
            )
            .separator()
            .toggle(
                self.t(Key::MenuAutomateVolume),
                MenuCommand::ShowAutomation(track, ParamTarget::TrackGain(track)),
                showing == Some(ParamTarget::TrackGain(track)),
            )
            .toggle(
                self.t(Key::MenuAutomatePan),
                MenuCommand::ShowAutomation(track, ParamTarget::TrackPan(track)),
                showing == Some(ParamTarget::TrackPan(track)),
            );

        // Clearing is offered only for a lane that exists: on a parameter nobody has automated it
        // is an item that can only do nothing, and a menu full of those is a menu people stop
        // reading.
        let menu = match showing.filter(|target| self.session.is_automated(*target)) {
            Some(target) => menu.item(
                self.t(Key::MenuClearAutomation),
                MenuCommand::ClearAutomation(target),
            ),
            None => menu,
        };

        // Only for a track that has something to freeze. On one with no generated clips it is a
        // row that can only report zero.
        let menu = match generated > 0 {
            true => menu.separator().item(
                self.t(Key::MenuFreezeTrack),
                MenuCommand::FreezeTrack(track),
            ),
            false => menu,
        };

        // The palette, as swatches. The colours carry the meaning and the rows are numbered
        // rather than named: the set holds two entries a reasonable person would call orange, and
        // naming those twice over in two languages is an argument nobody needs to have. The word
        // is on every row because this menu has no section headings and one row saying what the
        // run below it is would have to be a disabled item pretending to be a heading.
        let colour = self.t(Key::MenuTrackColor);
        let menu =
            Color::PALETTE
                .iter()
                .enumerate()
                .fold(menu.separator(), |menu, (index, color)| {
                    menu.colour(
                        format!("{colour} {}", index + 1),
                        MenuCommand::SetTrackColor(track, *color),
                        self.theme.track_color(color.0),
                        *color == current_color,
                    )
                });

        menu.separator()
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
    }

    /// The menu for an empty spot in a track's lane.
    pub(crate) fn lane_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        start: Ticks,
    ) -> ContextMenu {
        let Some(entry) = self.project().track(track) else {
            return self.arrangement_menu(anchor);
        };
        let is_instrument = entry.kind.as_instrument().is_some();
        ContextMenu::new(anchor, entry.name.clone())
            .item_if(
                is_instrument,
                self.t(Key::MenuNewClipHere),
                MenuCommand::NewClip { track, start },
            )
            .item_if(
                is_instrument,
                self.t(Key::MenuGenerateClip),
                MenuCommand::ShowPresetPicker {
                    track,
                    start,
                    anchor,
                },
            )
            // *Here* rather than Paste, because this is the one place a paste has a position
            // behind it: the pointer landed somewhere, and that is where the material goes
            // instead of wherever the playhead happens to be parked.
            .item_if(
                !self.session.clipboard().is_empty(),
                self.t(Key::MenuPasteHere),
                MenuCommand::PasteClips { track, at: start },
            )
            .separator()
            .item(
                self.t(Key::MenuDuplicateTrack),
                MenuCommand::DuplicateTrack(track),
            )
            .item(
                self.t(Key::MenuRenameTrack),
                MenuCommand::RenameTrack(track),
            )
            .item(self.t(Key::CmdDeleteTrack), MenuCommand::DeleteTrack(track))
            .separator()
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
    }

    /// The menu for the arrangement below the last track.
    pub(crate) fn arrangement_menu(&self, anchor: Point<Pixels>) -> ContextMenu {
        ContextMenu::new(anchor, self.t(Key::MenuArrangement))
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                MenuCommand::NewInstrumentTrack,
            )
            .item(self.t(Key::MenuNewAudioTrack), MenuCommand::NewAudioTrack)
            .item(self.t(Key::MenuNewBusTrack), MenuCommand::NewBusTrack)
    }

    /// The projects opened lately, as a menu.
    ///
    /// A context menu rather than a submenu of File, because gpui's menu rows carry an action
    /// and an action cannot carry a path — a recent list is nothing *but* paths. This one is
    /// opened by the File row and by the palette, so it reaches the same place either way.
    ///
    /// Each row shows the project's own name with the folder it sits in underneath the rest of
    /// the path: two projects called Demo are the ordinary case, and a list of identical names
    /// is a list nobody can choose from.
    pub(crate) fn recent_menu(&self, anchor: Point<Pixels>) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::CmdOpenRecent));
        if self.settings.recent.is_empty() {
            // A disabled row rather than an empty menu. A menu that opens with nothing in it
            // reads as a menu that failed.
            return menu.item_if(
                false,
                self.t(Key::MenuNoRecentProjects),
                MenuCommand::ForgetRecent,
            );
        }
        for path in &self.settings.recent {
            menu = menu.item(recent_label(path), MenuCommand::OpenRecent(path.clone()));
        }
        menu.separator()
            .item(self.t(Key::MenuForgetRecent), MenuCommand::ForgetRecent)
    }

    /// The menu for one parameter, wherever its control is drawn.
    ///
    /// Automation used to be asked for from the *track* menu, which could only name the two
    /// parameters a track has of its own — its fader and its pan. Everything else the document
    /// can automate, and the engine has always played back, was unreachable: a send level, a
    /// filter cutoff, an effect's mix. Asking the control itself is what makes the other
    /// hundred reachable without a menu that lists them, and it is where a hand already is.
    ///
    /// [`automation_offer`] decides what the automation rows say, including the case where they
    /// say nothing.
    pub(crate) fn param_menu(
        &self,
        anchor: Point<Pixels>,
        target: ParamTarget,
        title: impl Into<SharedString>,
    ) -> ContextMenu {
        let offer = automation_offer(
            target,
            &self.automation_lanes,
            self.session.is_automated(target),
        );
        let menu = ContextMenu::new(anchor, title)
            .item(
                self.t(Key::MenuSetValue),
                MenuCommand::SetParamValue(target),
            )
            .item(self.t(Key::MenuResetValue), MenuCommand::ResetParam(target));
        // Nothing at all for a master parameter rather than a disabled row: a row that can never
        // become usable teaches nothing by being there, and the master strip's controls would
        // carry two of them.
        let menu = match offer.lane {
            None => menu,
            Some(track) => menu.separator().toggle(
                self.t(Key::MenuAutomate),
                MenuCommand::ShowAutomation(track, target),
                offer.showing,
            ),
        };
        // The shape belongs to the lane, so it is only asked about once there is one. A parameter
        // with no points has nothing to get between.
        let menu = match self
            .session
            .automation()
            .lane(target)
            .map(|lane| lane.curve)
        {
            None => menu,
            Some(shape) => menu
                .separator()
                .toggle(
                    self.t(Key::MenuCurveLine),
                    MenuCommand::SetAutomationCurve {
                        target,
                        curve: AutomationCurve::Linear,
                    },
                    shape == AutomationCurve::Linear,
                )
                .toggle(
                    self.t(Key::MenuCurveStep),
                    MenuCommand::SetAutomationCurve {
                        target,
                        curve: AutomationCurve::Hold,
                    },
                    shape == AutomationCurve::Hold,
                )
                .separator(),
        };
        menu.item_if(
            offer.written,
            self.t(Key::MenuClearAutomation),
            MenuCommand::ClearAutomation(target),
        )
    }

    /// The named positions of a discrete parameter, aimed at one plugin's copy of it.
    ///
    /// A choice used to cycle on click, which is the right control for two positions and the wrong
    /// one for eight: reaching the pulse wave meant counting, and overshooting meant going round
    /// again. A menu names them all and ticks the one in force, which is what the preset and groove
    /// pickers already do with the same kind of question.
    pub(crate) fn param_choice_menu(
        &self,
        anchor: Point<Pixels>,
        target: ParamTarget,
        descriptor: &ParamDescriptor,
    ) -> ContextMenu {
        let current = self.session.param_value(target, descriptor);
        let mut menu = ContextMenu::new(anchor, self.param_label(&descriptor.name));
        for index in 0..descriptor.choices.len() {
            let value = crate::ui::plugin_editor::discrete_value(descriptor, index);
            menu = menu.toggle(
                self.format_param(descriptor, value),
                MenuCommand::SetParamChoice { target, value },
                // Compared as positions rather than as floats: a value read back out of a saved
                // document is whatever survived the round trip, not necessarily the integer that
                // went in.
                current.round() as i64 == value.round() as i64,
            );
        }
        menu
    }

    /// The menu for one effect in a chain.
    pub(crate) fn effect_menu(
        &mut self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
        slot: EffectSlotId,
        name: impl Into<SharedString>,
    ) -> ContextMenu {
        let enabled = self.session.effect_enabled(track, slot).unwrap_or(true);
        // Only where the plugin has somewhere to put a key. An effect with no reading for one
        // offered a source would be a row that does nothing, and the menu is short enough that
        // every row in it should mean something.
        let keyed = self.session.effect_wants_sidechain(track, slot);
        let mut menu = ContextMenu::new(anchor, name).toggle(
            self.t(Key::MenuEnabled),
            MenuCommand::ToggleEffect { track, slot },
            enabled,
        );
        if keyed {
            menu = menu.item(
                self.t(Key::MenuSidechain),
                MenuCommand::ShowSidechainPicker {
                    track,
                    slot,
                    at: anchor,
                },
            );
        }
        menu.separator()
            .item(
                self.t(Key::MenuMoveUp),
                MenuCommand::MoveEffect {
                    track,
                    slot,
                    delta: -1,
                },
            )
            .item(
                self.t(Key::MenuMoveDown),
                MenuCommand::MoveEffect {
                    track,
                    slot,
                    delta: 1,
                },
            )
            .item(self.t(Key::MenuRemove), MenuCommand::RemoveEffect(slot))
            .separator()
            .item(
                self.t(Key::MenuAddEffect),
                MenuCommand::ShowEffectPicker { track, at: anchor },
            )
    }

    /// Every track an effect could be keyed from, with a tick on the one it listens to now.
    ///
    /// The tracks that would close a loop are simply not here — the session leaves them out of
    /// [`Session::sidechain_sources`](auris_session::Session::sidechain_sources) — because a row
    /// that can only be refused is worse than no row. "None" leads, and is what a slot starts on.
    pub(crate) fn sidechain_menu(
        &self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
        slot: EffectSlotId,
    ) -> ContextMenu {
        let current = self.session.effect_sidechain(track, slot);
        let sources: Vec<(TrackId, String)> = self
            .session
            .sidechain_sources(track)
            .into_iter()
            .filter_map(|id| Some((id, self.project().track(id)?.name.clone())))
            .collect();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuSidechain)).toggle(
            self.t(Key::MenuSidechainNone),
            MenuCommand::SetEffectSidechain {
                track,
                slot,
                source: None,
            },
            current.is_none(),
        );
        for (id, name) in sources {
            menu = menu.toggle(
                name,
                MenuCommand::SetEffectSidechain {
                    track,
                    slot,
                    source: Some(id),
                },
                current == Some(id),
            );
        }
        menu
    }

    /// The device inputs a track could be recorded from.
    ///
    /// Every channel on its own and then every pair, because a microphone is one and a keyboard
    /// is two and the track does not know which it is about to be given. Numbered from one, the
    /// way the numbers are printed on the interface — everything below this line counts from zero
    /// and nobody choosing a socket does.
    ///
    /// Choosing one arms the track as well as pointing it, so this is a way to start recording
    /// and not only a way to adjust it. `&mut` because how many channels the device has is
    /// remembered rather than asked for; see
    /// [`Session::input_channel_count`](auris_session::Session::input_channel_count).
    pub(crate) fn input_menu(&mut self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let current = self.session.track_arm(track);
        let channels = self.session.input_channel_count();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuRecordInput))
            .toggle(
                self.t(Key::MenuInputOff),
                MenuCommand::SetTrackInput { track, input: None },
                current.is_none(),
            )
            .separator();
        for first in 0..channels {
            let input = InputChannels::mono(first);
            menu = menu.toggle(
                input_label(input),
                MenuCommand::SetTrackInput {
                    track,
                    input: Some(input),
                },
                current == Some(input),
            );
        }
        if channels > 1 {
            menu = menu.separator();
        }
        // Pairs from the odd-numbered channels only. An interface's stereo inputs are 1-2 and
        // 3-4; offering 2-3 as well would double the list to describe a cable nobody has.
        for first in (0..channels.saturating_sub(1)).step_by(2) {
            let input = InputChannels::stereo(first);
            menu = menu.toggle(
                input_label(input),
                MenuCommand::SetTrackInput {
                    track,
                    input: Some(input),
                },
                current == Some(input),
            );
        }
        menu
    }

    /// Every effect the registry knows, aimed at one particular strip.
    ///
    /// Aimed, rather than added "to whatever is selected". Two call sites used to reach the
    /// browser by moving the selection first — a track menu set it so the pick would land on that
    /// track, and the mixer's master strip *cleared* it so the pick would land on master. The
    /// second silently deselected whatever the user was working on, and the two pulled in
    /// opposite directions on the same piece of state. Carrying the target in the command
    /// removes the question.
    pub(crate) fn effect_picker_menu(
        &self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
    ) -> ContextMenu {
        let effects: Vec<(String, String)> = self
            .registry()
            .effects()
            .map(|descriptor| (descriptor.id.to_string(), descriptor.name.to_string()))
            .collect();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuAddEffect));
        for (id, name) in effects {
            let label = crate::ui::inspector::audio_name(self, &name);
            menu = menu.item(
                label,
                MenuCommand::AddEffect {
                    track,
                    effect_id: id,
                },
            );
        }
        menu
    }

    /// Where a track's output could go: the master, then every bus in the project.
    ///
    /// *Every* bus, not only the legal ones. A destination that would send a signal back into
    /// itself is greyed out rather than left out, because the two look identical to a person
    /// reading the list and mean different things — a bus that is missing reads as a bus that was
    /// never made. Which of the two a row is comes from
    /// [`Session::can_route`](auris_session::Session::can_route), the same rule the command that
    /// enforces it uses, so the list can never offer something the session would refuse.
    pub(crate) fn output_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let current = self
            .project()
            .track(track)
            .map(|track| track.output)
            .unwrap_or_default();
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuRouteTo)).toggle(
            self.t(Key::Master),
            MenuCommand::SetTrackOutput(track, Output::Master),
            current == Output::Master,
        );
        for (bus, name) in self.bus_names() {
            menu = menu.toggle_if(
                self.session.can_route(track, bus),
                name,
                MenuCommand::SetTrackOutput(track, Output::Bus(bus)),
                current == Output::Bus(bus),
            );
        }
        menu
    }

    /// Every bus a track could send to, with the ones that would loop greyed out for the reason
    /// [`Self::output_menu`] gives.
    ///
    /// A bus it already sends to is still offered: a second send to the same bus is unusual rather
    /// than wrong, and hiding it would mean the list changed shape as it was used.
    pub(crate) fn send_picker_menu(&self, anchor: Point<Pixels>, track: TrackId) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::MenuAddSend));
        for (bus, name) in self.bus_names() {
            menu = menu.item_if(
                self.session.can_route(track, bus),
                name,
                MenuCommand::AddSend { track, bus },
            );
        }
        menu
    }

    /// Every bus in the project, by id and name.
    ///
    /// Collected rather than borrowed: a menu row owns its label, and the walk over the track list
    /// has to finish before the menu is handed back.
    fn bus_names(&self) -> Vec<(TrackId, SharedString)> {
        self.session
            .buses()
            .map(|bus| (bus.id, SharedString::from(bus.name.clone())))
            .collect()
    }

    /// The menu for one send row in the mixer.
    pub(crate) fn send_menu(
        &self,
        anchor: Point<Pixels>,
        track: TrackId,
        send: SendId,
    ) -> ContextMenu {
        let pre_fader = self
            .project()
            .track(track)
            .and_then(|track| track.sends.iter().find(|existing| existing.id == send))
            .is_some_and(|send| send.pre_fader);
        ContextMenu::new(anchor, self.t(Key::Sends))
            .toggle(
                self.t(Key::MenuSendPreFader),
                MenuCommand::ToggleSendPreFader { track, send },
                pre_fader,
            )
            .item(
                self.t(Key::MenuRemoveSend),
                MenuCommand::RemoveSend { track, send },
            )
            .separator()
            .item(
                self.t(Key::MenuAddSend),
                MenuCommand::ShowSendPicker { track, at: anchor },
            )
    }
}

/// How one recent project is named in the menu.
///
/// The project's own name, and the folder holding it in brackets after it. Two projects called
/// Demo are the ordinary case — one in this month's folder and one in last year's — and a list
/// that showed only the name would be a list nobody can choose from. The whole path would be
/// truthful and unreadable: these are nested five folders deep and the interesting part is the
/// last one.
///
/// A free function so the rule can be asserted without a window.
pub(crate) fn recent_label(path: &std::path::Path) -> String {
    let name = path
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| path.to_string_lossy().into_owned());
    match path
        .parent()
        .and_then(|parent| parent.file_name())
        .map(|folder| folder.to_string_lossy().into_owned())
    {
        // A project folder is named after the project it holds, so this pair is usually the same
        // word twice — `Demo` in `Demo`. The folder above it is the one that distinguishes them.
        Some(folder) if folder != name => format!("{name}  ({folder})"),
        Some(_) => match path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.file_name())
        {
            Some(above) => format!("{name}  ({})", above.to_string_lossy()),
            None => name,
        },
        None => name,
    }
}

#[cfg(test)]
mod tests {
    use super::recent_label;
    use std::path::Path;

    #[test]
    fn a_recent_project_is_named_by_itself_and_where_it_lives() {
        // `Session::save_as` puts `Demo.auris` inside a folder called `Demo`, so the folder
        // right above it says nothing. The one above *that* is what tells two Demos apart.
        assert_eq!(
            recent_label(Path::new("/music/2026-08/Demo/Demo.auris")),
            "Demo  (2026-08)"
        );
        assert_eq!(
            recent_label(Path::new("/music/2025-01/Demo/Demo.auris")),
            "Demo  (2025-01)"
        );
    }

    #[test]
    fn a_project_not_in_a_folder_of_its_own_names_the_folder_it_is_in() {
        assert_eq!(
            recent_label(Path::new("/music/sketches/Riff.auris")),
            "Riff  (sketches)"
        );
    }

    #[test]
    fn a_project_with_nothing_above_it_is_just_itself() {
        assert_eq!(recent_label(Path::new("Riff.auris")), "Riff");
    }
}
