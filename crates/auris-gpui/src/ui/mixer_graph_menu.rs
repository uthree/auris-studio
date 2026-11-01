//! The mixer graph's two context menus: the one a node opens and the one the empty canvas opens.
//!
//! A node's menu is the menu that thing already has elsewhere — a strip is a track, so it gets
//! the track header's menu, and an insert gets the inspector's effect menu — because the same
//! object answering a right click two different ways is a thing to learn twice. The canvas has no
//! object under the pointer, so its menu is about adding one, and a node added from it appears
//! where the menu was opened rather than wherever the automatic layout would have put it.

use super::*;

/// A track the canvas menu can add.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NewMixerTrack {
    /// An instrument track playing the default instrument.
    Instrument,
    /// A drum track with the built-in kit.
    Drum,
    /// A singer track.
    Singer,
    /// An empty audio track.
    Audio,
    /// A summing bus.
    Bus,
}

impl AurisApp {
    /// What right-clicking `node` offers.
    pub(crate) fn mixer_node_menu(
        &mut self,
        at: gpui::Point<gpui::Pixels>,
        node: MixerNode,
    ) -> ContextMenu {
        match node {
            MixerNode::Strip(Some(track)) => {
                self.select_track(track);
                self.track_menu(at, track)
            }
            MixerNode::Strip(None) => ContextMenu::new(at, self.t(Key::Master)).item(
                self.t(Key::MenuAddEffect),
                MenuCommand::ShowEffectPicker { track: None, at },
            ),
            MixerNode::Effect(owner, slot) => {
                let effect_id = owner
                    .and_then(|id| self.project().track(id))
                    .map_or(&self.project().master, |track| &track.mixer)
                    .effects
                    .iter()
                    .find(|effect| effect.id == slot)
                    .map(|effect| effect.effect_id.clone());
                match effect_id {
                    Some(effect_id) => {
                        let label = self.effect_label(slot, &effect_id);
                        self.effect_menu(at, owner, slot, label)
                    }
                    None => ContextMenu::new(at, self.t(Key::MixerGraph)),
                }
            }
        }
    }

    /// What right-clicking the empty canvas offers: a new node there, or a fresh layout.
    pub(crate) fn mixer_canvas_menu(&self, at: gpui::Point<gpui::Pixels>) -> ContextMenu {
        let new = |kind| MenuCommand::NewMixerTrack { kind, at };
        ContextMenu::new(at, self.t(Key::MixerGraph))
            .item(
                self.t(Key::MenuNewInstrumentTrack),
                new(NewMixerTrack::Instrument),
            )
            .item(self.t(Key::MenuNewDrumTrack), new(NewMixerTrack::Drum))
            .item(self.t(Key::MenuNewSingerTrack), new(NewMixerTrack::Singer))
            .item(self.t(Key::MenuNewAudioTrack), new(NewMixerTrack::Audio))
            .item(self.t(Key::MenuNewBusTrack), new(NewMixerTrack::Bus))
            .separator()
            .item(self.t(Key::MixerLayout), MenuCommand::ArrangeMixerGraph)
    }

    /// Adds a track and places its strip at `at`, a window position over the canvas.
    pub(crate) fn add_mixer_track(
        &mut self,
        kind: NewMixerTrack,
        at: gpui::Point<gpui::Pixels>,
        cx: &mut gpui::Context<Self>,
    ) {
        let place = self.mixer_canvas.as_ref().map(|canvas| {
            let state = canvas.state.read(cx);
            state.viewport.screen_to_flow(
                f32::from(at.x) - state.origin.0,
                f32::from(at.y) - state.origin.1,
            )
        });
        let before = self.project().tracks.len();
        match kind {
            NewMixerTrack::Instrument => self.add_instrument_track(),
            NewMixerTrack::Drum => self.add_drum_track(),
            NewMixerTrack::Singer => self.add_singer_track(),
            NewMixerTrack::Audio => self.add_audio_track(),
            NewMixerTrack::Bus => self.add_bus_track(),
        }
        let added = (self.project().tracks.len() > before)
            .then_some(self.selected_track)
            .flatten();
        if let (Some(track), Some(place)) = (added, place) {
            let mut view = self.project().mixer_graph_view.clone();
            view.nodes.insert(
                MixerNode::Strip(Some(track)).key(),
                MixerNodePosition {
                    x: place.x,
                    y: place.y,
                },
            );
            self.session.set_mixer_graph_view(view);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::{Dock, Panel};
    use crate::harness::{open, paint, right_press};
    use gpui::{TestAppContext, point};

    fn show(this: &mut AurisApp) {
        this.panels.set_detached(Panel::Mixer, false);
        this.panels.move_to(Panel::Mixer, Dock::Bottom);
        this.panels.set_size(Dock::Bottom, px(600.));
        this.panels.show(Panel::Mixer);
    }

    /// The command behind the row labelled `label`.
    fn command_for(menu: &ContextMenu, label: &str) -> Option<MenuCommand> {
        menu.entries.iter().find_map(|entry| match entry {
            crate::ui::context_menu::MenuEntry::Item(item) if item.label.as_ref() == label => {
                Some(item.command.clone())
            }
            _ => None,
        })
    }

    #[gpui::test]
    fn right_clicking_a_strip_offers_the_track_menu_and_deletes_the_track(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let track = app.update(cx, |this, _| {
            show(this);
            this.session.add_bus_track("Group")
        });
        paint(&app, cx);
        let card = cx
            .debug_bounds(Box::leak(
                format!("graph-gain-{}", node_id(MixerNode::Strip(Some(track)))).into_boxed_str(),
            ))
            .expect("the strip's fader is drawn");
        // Above the fader, on the card itself rather than on a control.
        right_press(cx, point(card.center().x, card.top() - px(8.)));
        let delete = app.update(cx, |this, _| {
            let menu = this.menu.as_ref().expect("a menu opened");
            assert_eq!(menu.title.as_ref(), "Group");
            command_for(menu, this.t(Key::CmdDeleteTrack))
        });
        app.update(cx, |this, cx| {
            this.close_menu();
            this.run_menu_command(delete.expect("delete is offered"), cx);
        });
        app.update(cx, |this, _| assert!(this.project().track(track).is_none()));
    }

    #[gpui::test]
    fn right_clicking_an_insert_offers_its_effect_menu(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (track, slot) = app.update(cx, |this, _| {
            show(this);
            let track = this.session.add_bus_track("Group");
            let slot = this
                .session
                .add_effect(Some(track), "auris.fx.gain")
                .unwrap();
            (track, slot)
        });
        paint(&app, cx);
        let menu = app.update(cx, |this, _| {
            this.mixer_node_menu(
                point(px(10.), px(10.)),
                MixerNode::Effect(Some(track), slot),
            )
        });
        let remove = command_for(&menu, app.update(cx, |this, _| this.t(Key::MenuRemove)));
        assert_eq!(remove, Some(MenuCommand::RemoveEffect(slot)));
    }

    #[gpui::test]
    fn the_empty_canvas_adds_a_node_where_it_was_clicked(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| show(this));
        paint(&app, cx);
        let canvas = app.update(cx, |this, cx| {
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            state.canvas_bounds.expect("the canvas was measured")
        });
        // Far from every node: the master sits at the left of a fresh graph.
        let at = point(canvas.right() - px(40.), canvas.bottom() - px(40.));
        right_press(cx, at);
        let add = app.update(cx, |this, _| {
            let menu = this.menu.as_ref().expect("the canvas opened a menu");
            assert_eq!(menu.title.as_ref(), this.t(Key::MixerGraph));
            command_for(menu, this.t(Key::MenuNewBusTrack))
        });
        assert_eq!(
            add,
            Some(MenuCommand::NewMixerTrack {
                kind: NewMixerTrack::Bus,
                at
            })
        );
        let before = app.update(cx, |this, _| this.project().tracks.len());
        app.update(cx, |this, cx| {
            this.close_menu();
            this.run_menu_command(add.unwrap(), cx);
        });
        paint(&app, cx);
        app.update(cx, |this, cx| {
            assert_eq!(this.project().tracks.len(), before + 1);
            let bus = this.project().tracks.last().unwrap().id;
            let state = this.mixer_canvas.as_ref().unwrap().state.read(cx);
            let node = state
                .get_node(&node_id(MixerNode::Strip(Some(bus))))
                .expect("the bus is a node");
            let (x, y) = state.viewport.flow_to_screen(node.position);
            assert!((x + state.origin.0 - f32::from(at.x)).abs() < 0.5);
            assert!((y + state.origin.1 - f32::from(at.y)).abs() < 0.5);
        });
    }
}
