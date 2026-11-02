//! Simultaneous track monitoring in the central workspace.

use std::{cell::Cell, collections::HashMap, rc::Rc};

use auris_i18n::Key;
use auris_session::{MotionFrame, VisualizerFrame, prelude::*};
use gpui::{
    AnyElement, Bounds, Context, Pixels, ScrollHandle, Window, canvas, div, prelude::*, rems,
};

use super::{
    physical_model::model_thumbnail,
    scrollbars::ScrollPanel,
    visualizer::waveform_thumbnail,
    widgets::{ButtonStyle, button},
};
use crate::{app::AurisApp, theme::Metrics};

#[derive(Default)]
pub(crate) struct TrackGrid {
    pub(crate) open: bool,
    frozen: bool,
    pub(crate) scroll: ScrollHandle,
    bounds: Rc<Cell<Option<Bounds<Pixels>>>>,
    frames: HashMap<TrackId, TileFrame>,
}

struct TileFrame {
    instrument: Option<String>,
    data: TrackFrame,
}

enum TrackFrame {
    Model(Option<Box<MotionFrame>>),
    Audio(Option<VisualizerFrame>),
}

/// Tiles keep a readable minimum width and share all remaining space equally.
fn columns(width_in_rems: f32) -> u16 {
    ((width_in_rems - 1.5) / 19.0).floor().clamp(1., 12.) as u16
}

impl AurisApp {
    pub(crate) fn set_track_grid(&mut self, open: bool) {
        self.track_grid.open = open;
        if open {
            // Hidden timeline geometry must not turn a drop on a tile into a timeline edit.
            self.canvas.lanes.set(None);
            self.canvas.ruler.set(None);
            self.canvas.structure.set(None);
            self.canvas.harmony.set(None);
            self.canvas.lyrics.set(None);
        } else {
            self.track_grid.frozen = false;
            self.track_grid.frames.clear();
        }
        self.poll_track_grid();
    }

    pub(crate) fn poll_track_grid(&mut self) {
        let live = self.track_grid.open && !self.track_grid.frozen;
        self.session.watch_track_overview(live);
        if !live {
            let project = self.session.project();
            self.track_grid.frames.retain(|id, frame| {
                project
                    .track(*id)
                    .is_some_and(|track| instrument_id(&track.kind) == frame.instrument.as_deref())
            });
            return;
        }
        self.track_grid.frames = self
            .project()
            .tracks
            .iter()
            .map(|track| {
                let frame = if track.kind.as_instrument().is_some()
                    && self.session.has_instrument_motion(track.id)
                {
                    TrackFrame::Model(self.session.instrument_motion(track.id).map(Box::new))
                } else {
                    TrackFrame::Audio(self.session.track_overview_frame(track.id))
                };
                (
                    track.id,
                    TileFrame {
                        instrument: instrument_id(&track.kind).map(str::to_owned),
                        data: frame,
                    },
                )
            })
            .collect();
    }

    pub(crate) fn render_track_workspace(
        &mut self,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let theme = self.theme.clone();
        let controls = div()
            .flex()
            .flex_shrink_0()
            .flex_wrap()
            .items_center()
            .gap_2()
            .p_2()
            .border_b_1()
            .border_color(theme.border_subtle)
            .child(button(
                "track-grid-freeze",
                self.t(Key::VisualizerFreeze),
                ButtonStyle::Normal,
                self.track_grid.frozen,
                theme.accent,
                &theme,
                cx.listener(|this, _, _, cx| {
                    this.track_grid.frozen = !this.track_grid.frozen;
                    this.poll_track_grid();
                    cx.notify();
                }),
            ));
        let content = if self.track_grid.open {
            self.render_track_grid(window, cx)
        } else {
            self.render_arrangement(window, cx).into_any_element()
        };
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_w_0()
            .min_h(crate::dock::PanelLayout::MIN_ARRANGEMENT)
            .bg(theme.surface)
            .when(self.track_grid.open, |workspace| workspace.child(controls))
            .child(content)
            .into_any_element()
    }

    fn render_track_grid(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let width = self
            .track_grid
            .bounds
            .get()
            .map_or(window.rem_size() * 19., |bounds| bounds.size.width);
        let count = columns(f32::from(width) / f32::from(window.rem_size()));
        let tracks: Vec<_> = self.project().tracks.iter().map(|track| track.id).collect();
        let tiles: Vec<_> = tracks
            .into_iter()
            .map(|id| self.track_tile(id, cx))
            .collect();
        let bounds = Rc::clone(&self.track_grid.bounds);
        let empty = tiles.is_empty();
        let body = div()
            .id("track-grid-body")
            .w_full()
            .overflow_y_scroll()
            .when(!empty, |body| {
                body.child(div().grid().grid_cols(count).gap_3().p_3().children(tiles))
            })
            .when(empty, |body| {
                body.child(
                    div()
                        .flex()
                        .flex_col()
                        .items_start()
                        .gap_3()
                        .p_4()
                        .text_sm()
                        .text_color(self.theme.text_muted)
                        .child(self.t(Key::TrackGridEmpty))
                        .child(button(
                            "track-grid-add",
                            self.t(Key::CmdAddInstrumentTrack),
                            ButtonStyle::Normal,
                            false,
                            self.theme.accent,
                            &self.theme,
                            cx.listener(|this, _, _, cx| {
                                this.add_instrument_track();
                                this.poll_track_grid();
                                cx.notify();
                            }),
                        )),
                )
            })
            .on_mouse_down(
                gpui::MouseButton::Right,
                Self::opens_menu(cx, |this, at| this.arrangement_menu(at)),
            );
        div()
            .id("track-grid")
            .debug_selector(|| "track-grid".into())
            .relative()
            .flex()
            .flex_1()
            .min_w_0()
            .min_h_0()
            .child(
                canvas(move |at, _, _| bounds.set(Some(at)), |_, _, _, _| ())
                    .absolute()
                    .size_full(),
            )
            .child(self.scrolling(ScrollPanel::TrackGrid, body, cx))
            .into_any_element()
    }

    fn track_tile(&self, id: TrackId, cx: &mut Context<Self>) -> AnyElement {
        let Some(track) = self.project().track(id) else {
            return div().into_any_element();
        };
        let theme = &self.theme;
        let instrument = instrument_id(&track.kind);
        let label = instrument.map_or_else(
            || self.t(crate::i18n::track_kind_key(&track.kind)).to_owned(),
            |instrument| self.instrument_label(id, instrument),
        );
        let selected = self.selected_track == Some(id);
        let frame = self
            .track_grid
            .frames
            .get(&id)
            .filter(|frame| frame.instrument.as_deref() == instrument)
            .map(|frame| &frame.data);
        let model = track.kind.as_instrument().is_some() && self.session.has_instrument_motion(id);
        let visualization = if model {
            let motion = match frame {
                Some(TrackFrame::Model(frame)) => frame.as_deref().copied(),
                _ => None,
            };
            model_thumbnail(
                instrument.unwrap_or_default().to_owned(),
                motion,
                self.language,
                theme.clone(),
            )
        } else {
            let audio = match frame {
                Some(TrackFrame::Audio(frame)) => frame.clone(),
                _ => None,
            };
            waveform_thumbnail(audio, theme.clone())
        };
        let waiting = match frame {
            Some(TrackFrame::Model(frame)) => frame.is_none(),
            Some(TrackFrame::Audio(frame)) => frame.is_none(),
            None => true,
        };
        let header = button(
            ("track-grid-select", id.0),
            track.name.clone(),
            ButtonStyle::Ghost,
            selected,
            theme.accent,
            theme,
            cx.listener(move |this, _, _, cx| {
                this.select_track(id);
                cx.notify();
            }),
        )
        .w_full()
        .min_w_0()
        .overflow_hidden()
        .tooltip(super::tooltip::keyed_tip(track.name.clone(), "", theme));
        let mut controls = div().flex().gap_2();
        for (key, muted) in [(Key::Mute, true), (Key::Solo, false)] {
            controls = controls.child(button(
                (
                    if muted {
                        "track-grid-mute"
                    } else {
                        "track-grid-solo"
                    },
                    id.0,
                ),
                self.t(key),
                ButtonStyle::Normal,
                if muted {
                    track.mixer.mute
                } else {
                    track.mixer.solo
                },
                if muted { theme.mute } else { theme.solo },
                theme,
                cx.listener(move |this, _, _, cx| {
                    if muted {
                        this.toggle_mute(id);
                    } else {
                        this.toggle_solo(id);
                    }
                    cx.notify();
                }),
            ));
        }
        div()
            .id(("track-grid-tile", id.0))
            .debug_selector(move || format!("track-grid-tile-{}", id.0))
            .min_w_0()
            .flex()
            .flex_col()
            .gap_2()
            .p_2()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface)
            .border_1()
            .border_color(if selected {
                theme.accent
            } else {
                theme.border_subtle
            })
            .on_mouse_down(
                gpui::MouseButton::Right,
                Self::opens_menu(cx, move |this, at| {
                    this.select_track(id);
                    this.track_menu(at, id)
                }),
            )
            .child(header)
            .child(
                div()
                    .id(("track-grid-instrument", id.0))
                    .text_xs()
                    .text_color(theme.text_muted)
                    .truncate()
                    .child(label.clone())
                    .tooltip(super::tooltip::keyed_tip(label, "", theme)),
            )
            .child(
                div()
                    .id((
                        if model {
                            "track-grid-model"
                        } else {
                            "track-grid-waveform"
                        },
                        id.0,
                    ))
                    .debug_selector(move || {
                        format!(
                            "track-grid-{}-{}",
                            if model { "model" } else { "waveform" },
                            id.0
                        )
                    })
                    .h(rems(11.))
                    .w_full()
                    .overflow_hidden()
                    .bg(theme.surface_sunken)
                    .child(visualization),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(if waiting {
                        Key::VisualizerWaiting
                    } else if model {
                        Key::PhysicalMotion
                    } else {
                        Key::VisualizerViewOscilloscope
                    })),
            )
            .child(controls)
            .into_any_element()
    }
}

fn instrument_id(kind: &TrackKind) -> Option<&str> {
    match kind {
        TrackKind::Instrument(track) | TrackKind::Drum(track) => Some(&track.instrument_id),
        TrackKind::Singer(track) => Some(&track.instrument_id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{self, click, paint};
    use gpui::{Modifiers, TestAppContext, VisualTestContext, px, size};

    fn click_track(prefix: &str, track: TrackId, cx: &mut VisualTestContext) {
        let bounds = cx
            .debug_bounds(format!("{prefix}-{}", track.0).leak())
            .unwrap();
        cx.simulate_click(bounds.center(), Modifiers::none());
    }

    fn toggle_grid_from_palette(cx: &mut VisualTestContext) {
        cx.simulate_keystrokes("secondary-shift-p");
        cx.simulate_input("Track grid");
        cx.simulate_keystrokes("enter");
        cx.run_until_parked();
    }

    #[gpui::test]
    fn grid_is_accessible_through_the_palette_without_visible_switches_or_a_default_shortcut(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = harness::open(cx);
        let action = crate::actions::BINDABLE
            .iter()
            .find(|action| action.id == "view.track_grid")
            .unwrap();
        assert!(action.default.is_none());
        assert!(cx.debug_bounds("track-view-grid").is_none());
        assert!(cx.debug_bounds("track-view-timeline").is_none());
        for language in [
            auris_i18n::Language::English,
            auris_i18n::Language::Japanese,
        ] {
            app.update(cx, |app, _| app.language = language);
            paint(&app, cx);
            app.read_with(cx, |app, _| {
                assert!(
                    app.menu_model()
                        .iter()
                        .flat_map(|menu| &menu.rows)
                        .all(|row| {
                            !matches!(
                                row,
                                crate::menu::MenuRow::Command {
                                    binding: "view.track_grid",
                                    ..
                                }
                            )
                        })
                );
            });
            toggle_grid_from_palette(cx);
            app.read_with(cx, |app, _| assert!(app.track_grid.open));
            toggle_grid_from_palette(cx);
            app.read_with(cx, |app, _| assert!(!app.track_grid.open));
        }
    }

    #[test]
    fn columns_keep_tiles_readable_at_narrow_and_zoomed_widths() {
        assert_eq!(columns(12.), 1);
        assert_eq!(columns(39.5), 2);
        assert_eq!(columns(58.5), 3);
        assert_eq!(columns(10000.), 12);
    }

    #[gpui::test]
    fn empty_grid_adds_a_track_without_leaving_the_view(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        app.update(cx, |app, _| {
            let tracks: Vec<_> = app.project().tracks.iter().map(|track| track.id).collect();
            for track in tracks {
                app.session.remove_track(track).unwrap();
            }
            app.selected_track = None;
        });
        paint(&app, cx);
        toggle_grid_from_palette(cx);
        paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(app.track_grid.open);
            assert!(app.project().tracks.is_empty());
        });
        click("track-grid-add", cx);
        paint(&app, cx);
        let track = app.read_with(cx, |app, _| {
            assert!(app.track_grid.open);
            assert_eq!(app.project().tracks.len(), 1);
            app.project().tracks[0].id
        });
        assert!(
            cx.debug_bounds(format!("track-grid-tile-{}", track.0).leak())
                .is_some()
        );
    }

    #[gpui::test]
    fn grid_shows_every_track_with_its_own_projection_and_controls(cx: &mut TestAppContext) {
        let (app, cx) = harness::open(cx);
        let (physical, chip, audio, bus) = app.update(cx, |app, _| {
            let physical = app
                .session
                .add_instrument_track("Violin", "auris.physical.violin")
                .unwrap();
            let chip = app
                .session
                .add_instrument_track("Chip", "auris.synth.chiptune")
                .unwrap();
            let audio = app.session.add_audio_track("Audio");
            let bus = app.session.add_bus_track("Bus");
            (physical, chip, audio, bus)
        });
        paint(&app, cx);
        let before = app.read_with(cx, |app, _| app.project().clone());
        toggle_grid_from_palette(cx);
        paint(&app, cx);
        app.read_with(cx, |app, _| assert!(app.canvas.lanes.get().is_none()));
        assert!(
            cx.debug_bounds(format!("track-grid-model-{}", physical.0).leak())
                .is_some()
        );
        for id in [chip, audio, bus] {
            assert!(
                cx.debug_bounds(format!("track-grid-waveform-{}", id.0).leak())
                    .is_some()
            );
        }
        click_track("track-grid-select", chip, cx);
        app.read_with(cx, |app, _| {
            assert_eq!(app.selected_track, Some(chip));
            assert_eq!(app.project(), &before);
        });
        click_track("track-grid-mute", audio, cx);
        click_track("track-grid-solo", chip, cx);
        app.read_with(cx, |app, _| {
            assert!(app.project().track(audio).unwrap().mixer.mute);
            assert!(app.project().track(chip).unwrap().mixer.solo);
        });
        click("track-grid-freeze", cx);
        app.read_with(cx, |app, _| assert!(app.track_grid.frozen));
        toggle_grid_from_palette(cx);
        paint(&app, cx);
        app.read_with(cx, |app, _| {
            assert!(!app.track_grid.open);
            assert!(!app.track_grid.frozen);
            assert!(app.track_grid.frames.is_empty());
        });
        app.read_with(cx, |app, _| assert!(app.canvas.lanes.get().is_some()));
        cx.simulate_keystrokes("secondary-alt-g");
        app.read_with(cx, |app, _| assert!(!app.track_grid.open));
        toggle_grid_from_palette(cx);
        app.read_with(cx, |app, _| assert!(app.track_grid.open));
    }

    #[gpui::test]
    fn grid_reflows_with_the_available_region_and_clears_replaced_frozen_sources(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = harness::open(cx);
        let tracks = app.update(cx, |app, _| {
            (0..4)
                .map(|index| {
                    app.session
                        .add_default_instrument_track(format!("Track {index}"))
                        .unwrap()
                })
                .collect::<Vec<_>>()
        });
        toggle_grid_from_palette(cx);
        for width in [1920., 900., 640.] {
            harness::resize(&app, cx, size(px(width), px(900.)));
            // A second layout consumes the region measured by the first.
            paint(&app, cx);
            let region = cx.debug_bounds("track-grid").unwrap();
            let first = cx
                .debug_bounds(format!("track-grid-tile-{}", tracks[0].0).leak())
                .unwrap();
            assert!(first.left() >= region.left());
            assert!(first.right() <= region.right());
        }
        click("track-grid-freeze", cx);
        app.update(cx, |app, _| {
            app.session
                .set_track_instrument(tracks[0], "auris.physical.guitar")
                .unwrap();
            app.session.remove_track(tracks[1]).unwrap();
            app.poll_track_grid();
            assert!(!app.track_grid.frames.contains_key(&tracks[0]));
            assert!(!app.track_grid.frames.contains_key(&tracks[1]));
        });
        paint(&app, cx);
        assert!(
            cx.debug_bounds(format!("track-grid-model-{}", tracks[0].0).leak())
                .is_some()
        );
        app.read_with(cx, |app, _| {
            assert!(app.project().track(tracks[1]).is_none())
        });
    }
}
