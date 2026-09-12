//! Library timbre exploration with a background session job and explicit adoption.

use crate::{
    app::AurisApp,
    theme::Theme,
    ui::widgets::{ButtonStyle, button},
};
use auris_i18n::Key;
use auris_session::{TimbreMap, TimbreMapControl, prelude::*};
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*, px, relative};
use std::sync::Arc;

#[derive(Default)]
pub(crate) struct TimbreMapState {
    pub(crate) open: bool,
    pub(crate) map: Option<Arc<TimbreMap>>,
    pub(crate) control: Option<TimbreMapControl>,
    generation: u64,
    total: usize,
    selected: Option<usize>,
    preview_track: Option<TrackId>,
    error: Option<String>,
}

impl TimbreMapState {
    pub(crate) fn cancel(&mut self) {
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
    }
}

fn cluster_color(cluster: usize, theme: &Theme) -> gpui::Hsla {
    theme.group_color(cluster as f32 * 0.618_034)
}

impl AurisApp {
    pub(crate) fn close_timbre_map(&mut self) {
        self.timbre_map.cancel();
        self.timbre_map.open = false;
        if let Some(track) = self.timbre_map.preview_track.take() {
            self.session.stop_singer_preview(track);
        }
    }

    pub(crate) fn scan_timbre_map(&mut self, cx: &mut Context<Self>) {
        self.timbre_map.cancel();
        self.timbre_map.selected = None;
        self.timbre_map.error = None;
        if let Some(track) = self.timbre_map.preview_track.take() {
            self.session.stop_singer_preview(track);
        }
        self.timbre_map.map = None;
        let job = match self.session.timbre_map_job() {
            Ok(job) => job,
            Err(e) => {
                self.timbre_map.error = Some(e.to_string());
                return;
            }
        };
        self.timbre_map.total = job.sound_count();
        let generation = self.timbre_map.generation;
        let control = TimbreMapControl::default();
        self.timbre_map.control = Some(control.clone());
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job.run(&control) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.timbre_map.generation != generation {
                    return;
                }
                this.timbre_map.control = None;
                match result {
                    Ok(map) => this.timbre_map.map = Some(Arc::new(map)),
                    Err(e) => this.timbre_map.error = Some(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(200))
                    .await;
                let active = this
                    .update(cx, |this, cx| {
                        let active = this.timbre_map.generation == generation
                            && this.timbre_map.control.is_some();
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

    fn select_timbre(&mut self, index: usize) {
        let Some(map) = self.timbre_map.map.clone() else {
            return;
        };
        if index >= map.sounds.len() {
            return;
        }
        self.timbre_map.selected = Some(index);
        self.timbre_map.error = None;
        if let Some(track) = self.timbre_map.preview_track.take() {
            self.session.stop_singer_preview(track);
        }
        if let Some(track) = self.session.audition_track(self.selected_track) {
            match self.session.preview_timbre(&map, index, track) {
                Ok(()) => self.timbre_map.preview_track = Some(track),
                Err(e) => self.timbre_map.error = Some(e.to_string()),
            }
        } else {
            self.timbre_map.error = Some(self.t(Key::LibraryNeedsInstrumentTrack).into());
        }
    }

    fn adopt_timbre(&mut self) {
        let Some(sound) = self
            .timbre_map
            .map
            .as_ref()
            .and_then(|m| self.timbre_map.selected.and_then(|i| m.sounds.get(i)))
            .cloned()
        else {
            return;
        };
        let Some(track) = self.selected_track else {
            self.timbre_map.error = Some(self.t(Key::LibraryNeedsInstrumentTrack).into());
            return;
        };
        match self.session.use_timbre_sound(track, &sound) {
            Ok(()) => self.timbre_map.error = None,
            Err(e) => self.timbre_map.error = Some(e.to_string()),
        }
    }

    pub(crate) fn render_timbre_map(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.timbre_map.open {
            return None;
        }
        let theme = &self.theme;
        let mut body = div().flex().flex_col().gap_2().min_h_0();
        if let Some(control) = &self.timbre_map.control {
            body = body
                .child(format!(
                    "{} {}/{}",
                    self.t(Key::AnalysisRunning),
                    control.completed(),
                    self.timbre_map.total
                ))
                .child(button(
                    "timbre-cancel",
                    self.t(Key::Cancel),
                    ButtonStyle::Normal,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(|this, _, _, cx| {
                        this.timbre_map.cancel();
                        cx.notify();
                    }),
                ));
        }
        if let Some(error) = &self.timbre_map.error {
            body = body.child(error.clone());
        }
        if let Some(map) = &self.timbre_map.map {
            if map.sounds.is_empty() {
                body = body.child(self.t(Key::TimbreMapEmpty));
            }
            let positions = plot_positions(map.positions());
            let neighbors = self
                .timbre_map
                .selected
                .map(|i| map.nearest(i, 8))
                .unwrap_or_default();
            let mut plot = div()
                .id("timbre-plot")
                .relative()
                .w_full()
                .h(px(290.0))
                .flex_shrink_0()
                .bg(theme.surface_sunken)
                .border_1()
                .border_color(theme.border)
                .overflow_hidden();
            for (i, position) in positions.iter().enumerate() {
                let selected = self.timbre_map.selected == Some(i);
                let nearby = neighbors.iter().any(|(index, _)| *index == i);
                let color = cluster_color(map.clusters()[i], theme);
                plot = plot.child(
                    div()
                        .id(("timbre-point", i))
                        .debug_selector(move || format!("timbre-point-{i}"))
                        .absolute()
                        .left(relative(position[0]))
                        .top(relative(position[1]))
                        .size(px(if selected { 16.0 } else { 11.0 }))
                        .rounded_full()
                        .bg(color)
                        .border_2()
                        .border_color(if selected {
                            theme.text
                        } else if nearby {
                            theme.accent
                        } else {
                            color
                        })
                        .cursor_pointer()
                        .tooltip(crate::ui::tooltip::keyed_tip(
                            map.sounds[i].name.clone(),
                            "",
                            theme,
                        ))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.select_timbre(i);
                            cx.notify();
                        })),
                );
            }
            body = body
                .child(plot)
                .child(div().text_xs().text_color(theme.text_muted).child(format!(
                    "{} · {}: {:.0}% · {}: {}",
                    self.t(Key::TimbreMapGeometry),
                    self.t(Key::TimbreMapVariance),
                    map.explained_variance() * 100.0,
                    self.t(Key::TimbreMapSkipped),
                    map.skipped.len()
                )));
            if let Some(index) = self.timbre_map.selected {
                body = body.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .child(map.sounds[index].name.clone()),
                        )
                        .child(button(
                            "timbre-use",
                            self.t(Key::TimbreMapUse),
                            ButtonStyle::Primary,
                            false,
                            theme.accent,
                            theme,
                            cx.listener(|this, _, _, cx| {
                                this.adopt_timbre();
                                cx.notify();
                            }),
                        )),
                );
            }
            if let Some(track) = self
                .session
                .audition_track(self.selected_track)
                .and_then(|id| self.project().track(id))
            {
                body = body.child(div().text_xs().text_color(theme.text_muted).child(format!(
                    "{}: {}",
                    self.t(Key::TimbreMapAudition),
                    track.name
                )));
            }
            let mut lists = div().flex().gap_3().h(px(145.0)).min_h_0();
            for (id, heading, entries) in [
                (
                    "timbre-neighbors",
                    Key::TimbreMapNearest,
                    neighbors
                        .iter()
                        .map(|(i, d)| (*i, format!("{:.2} · {}", d, map.sounds[*i].name)))
                        .collect::<Vec<_>>(),
                ),
                (
                    "timbre-all",
                    Key::TimbreMapAll,
                    map.sounds
                        .iter()
                        .enumerate()
                        .map(|(i, s)| (i, s.name.clone()))
                        .collect(),
                ),
            ] {
                lists = lists.child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(self.t(heading))
                        .child(div().id(id).overflow_y_scroll().min_h_0().children(
                            entries.into_iter().map(|(i, label)| {
                                button(
                                    SharedString::from(format!("{id}-{i}")),
                                    label,
                                    ButtonStyle::Ghost,
                                    self.timbre_map.selected == Some(i),
                                    theme.accent,
                                    theme,
                                    cx.listener(move |this, _, _, cx| {
                                        this.select_timbre(i);
                                        cx.notify();
                                    }),
                                )
                                .w_full()
                                .justify_start()
                                .into_any_element()
                            }),
                        )),
                );
            }
            body = body.child(lists);
        }
        Some(
            div()
                .id("timbre-map")
                .size_full()
                .flex()
                .flex_col()
                .gap_2()
                .p_3()
                .bg(theme.surface)
                .text_color(theme.text)
                .border_1()
                .border_color(theme.border)
                .shadow_lg()
                .occlude()
                .on_mouse_down(gpui::MouseButton::Left, |_, _, cx| cx.stop_propagation())
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .child(self.t(Key::TimbreMap))
                        .child(
                            div()
                                .flex()
                                .gap_2()
                                .child(button(
                                    "timbre-rescan",
                                    self.t(Key::TimbreMapRescan),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.scan_timbre_map(cx);
                                        cx.notify();
                                    }),
                                ))
                                .child(button(
                                    "timbre-close",
                                    self.t(Key::Close),
                                    ButtonStyle::Normal,
                                    false,
                                    theme.accent,
                                    theme,
                                    cx.listener(|this, _, _, cx| {
                                        this.close_timbre_map();
                                        cx.notify();
                                    }),
                                )),
                        ),
                )
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::TimbreMapHint)),
                )
                .child(
                    div()
                        .id("timbre-map-body")
                        .overflow_y_scroll()
                        .min_h_0()
                        .child(body),
                )
                .into_any_element(),
        )
    }
}

fn plot_positions(positions: &[[f64; 2]]) -> Vec<[f32; 2]> {
    let mut low = [f64::INFINITY; 2];
    let mut high = [f64::NEG_INFINITY; 2];
    for p in positions {
        for axis in 0..2 {
            low[axis] = low[axis].min(p[axis]);
            high[axis] = high[axis].max(p[axis]);
        }
    }
    let span = (high[0] - low[0]).max(high[1] - low[1]).max(1e-9);
    positions
        .iter()
        .map(|p| {
            std::array::from_fn(|axis| {
                (0.48 + 0.88 * (p[axis] - (low[axis] + high[axis]) * 0.5) / span) as f32
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint};
    use crate::theme::{SCHEMES, Theme};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    fn map_window(app: &Entity<AurisApp>, cx: &mut VisualTestContext) -> VisualTestContext {
        let handle = app.read_with(cx, |app, _| {
            app.auxiliary_windows[&crate::auxiliary_window::Surface::TimbreMap]
        });
        VisualTestContext::from_window(handle.into(), cx)
    }

    #[test]
    fn cluster_colours_follow_every_theme_group_palette() {
        for scheme in SCHEMES {
            let theme = Theme::from_scheme(scheme);
            for cluster in 0..8 {
                assert_eq!(
                    cluster_color(cluster, &theme),
                    theme.group_color(cluster as f32 * 0.618_034),
                    "{} cluster {cluster}",
                    scheme.name
                );
            }
        }
    }

    #[gpui::test]
    fn browsing_is_read_only_and_adoption_is_undoable(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (before, index) = app.update(cx, |this, _| {
            let track = this
                .session
                .add_default_instrument_track("Map target")
                .unwrap();
            this.select_track(track);
            let map = this
                .session
                .timbre_map_job()
                .unwrap()
                .run(&TimbreMapControl::default())
                .unwrap();
            let index = map
                .sounds
                .iter()
                .position(|s| s.instrument_id == "auris.synth.fm2")
                .unwrap();
            this.timbre_map.map = Some(Arc::new(map));
            this.timbre_map.open = true;
            (this.project().clone(), index)
        });
        paint(&app, cx);
        // GPUI's debug selector API requires a static string, including for dynamic rows.
        let mut utility = map_window(&app, cx);
        click(
            Box::leak(format!("timbre-all-{index}").into_boxed_str()),
            &mut utility,
        );
        app.read_with(cx, |this, _| {
            assert_eq!(this.project(), &before);
            assert_eq!(this.timbre_map.selected, Some(index));
        });
        paint(&app, cx);
        click("timbre-use", &mut utility);
        app.update(cx, |this, _| {
            assert_ne!(this.project(), &before);
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
        click("timbre-close", &mut utility);
        app.read_with(cx, |this, _| assert!(!this.timbre_map.open));
    }

    #[gpui::test]
    fn closing_and_replacing_the_document_invalidate_pending_results(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let generation = app.update(cx, |this, _| {
            this.timbre_map.open = true;
            this.timbre_map.control = Some(TimbreMapControl::default());
            this.timbre_map.generation
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        click("timbre-close", &mut utility);
        app.update(cx, |this, _| {
            assert_ne!(this.timbre_map.generation, generation);
            assert!(this.timbre_map.control.is_none());
            this.timbre_map.open = true;
            this.timbre_map.control = Some(TimbreMapControl::default());
            let generation = this.timbre_map.generation;
            this.reset_view();
            assert!(!this.timbre_map.open);
            assert!(this.timbre_map.control.is_none());
            assert_ne!(this.timbre_map.generation, generation);
        });
    }
}
