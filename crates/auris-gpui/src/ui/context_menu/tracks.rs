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
            )
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
        &self,
        anchor: Point<Pixels>,
        track: Option<TrackId>,
        slot: EffectSlotId,
        name: impl Into<SharedString>,
    ) -> ContextMenu {
        let enabled = self.session.effect_enabled(track, slot).unwrap_or(true);
        ContextMenu::new(anchor, name)
            .toggle(
                self.t(Key::MenuEnabled),
                MenuCommand::ToggleEffect { track, slot },
                enabled,
            )
            .separator()
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
