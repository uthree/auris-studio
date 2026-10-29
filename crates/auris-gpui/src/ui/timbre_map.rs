//! Library timbre exploration with a background session job and explicit adoption.

use crate::{app::AurisApp, theme::Theme};
use atelier::plot::Plot3d;
use atelier::{
    Button, ButtonVariant, Card, ControlSize, EmptyState, IconName, Label, ListItem, ProgressBar,
    SectionHeader, Segment, SegmentedControl, StatusBar, StatusDot, TextColor, TextSize,
};
use auris_agent::timbre_names::{ClusterName, TimbreNamer};
use auris_i18n::Key;
use auris_session::{TimbreMap, TimbreMapControl, prelude::*};
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};
use std::sync::Arc;
#[path = "timbre_plot.rs"]
mod plot;
use plot::{TimbreCamera, plot_data};

#[derive(Default)]
pub(crate) struct TimbreMapState {
    pub(crate) open: bool,
    camera: TimbreCamera,
    hovered: Option<usize>,
    list_nearest: bool,
    pub(crate) map: Option<Arc<TimbreMap>>,
    pub(crate) control: Option<TimbreMapControl>,
    generation: u64,
    total: usize,
    selected: Option<usize>,
    preview_track: Option<TrackId>,
    error: Option<String>,
    names: Vec<ClusterName>,
    namer: Option<TimbreNamer>,
    naming_generation: u64,
    naming_error: Option<String>,
}

impl TimbreMapState {
    pub(crate) fn clear(&mut self) {
        self.cancel();
        self.map = None;
        self.names.clear();
        self.naming_error = None;
    }

    pub(crate) fn cancel(&mut self) {
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        self.generation = self.generation.wrapping_add(1);
        self.cancel_naming();
    }

    fn cancel_naming(&mut self) {
        self.namer = None;
        self.naming_generation = self.naming_generation.wrapping_add(1);
    }

    fn finish_naming(&mut self, generation: u64, result: Result<Vec<ClusterName>, String>) -> bool {
        if self.naming_generation != generation || self.map.is_none() {
            return false;
        }
        self.namer = None;
        match result {
            Ok(names) => self.names = names,
            Err(error) => self.naming_error = Some(error),
        }
        true
    }
}

fn cluster_color(cluster: usize, theme: &Theme) -> gpui::Hsla {
    theme.group_color(cluster as f32 * 0.618_034)
}

impl AurisApp {
    fn timbre_cluster_name(&self, cluster: usize) -> String {
        if let Some(name) = self.timbre_map.names.iter().find(|name| name.id == cluster) {
            return name.name.clone();
        }
        self.timbre_map
            .map
            .as_ref()
            .and_then(|map| {
                map.cluster_summaries()
                    .iter()
                    .find(|summary| summary.id == cluster)
            })
            .map(|summary| super::inspector::audio_name(self, &summary.representative.name))
            .unwrap_or_default()
    }

    /// Cluster annotation for an exact measured library choice, shared by tree and search rows.
    pub(crate) fn library_timbre_cluster(
        &self,
        instrument_id: &str,
        preset: Option<PresetRef>,
    ) -> Option<(gpui::Hsla, String)> {
        let cluster = self
            .timbre_map
            .map
            .as_ref()?
            .cluster_for(instrument_id, preset)?;
        Some((
            cluster_color(cluster, &self.theme),
            self.timbre_cluster_name(cluster),
        ))
    }

    fn name_timbre_clusters(&mut self, cx: &mut Context<Self>) {
        let Some(map) = &self.timbre_map.map else {
            return;
        };
        if self.timbre_map.namer.is_some() || self.timbre_map.control.is_some() {
            return;
        }
        let result = TimbreNamer::start(
            self.settings.agent.clone(),
            map.cluster_summaries(),
            self.language.code(),
        );
        self.timbre_map.naming_error = None;
        match result {
            Ok(namer) => self.timbre_map.namer = Some(namer),
            Err(error) => {
                self.timbre_map.naming_error = Some(error);
                return;
            }
        }
        let generation = self.timbre_map.naming_generation;
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
                let active = this
                    .update(cx, |this, cx| {
                        if this.timbre_map.naming_generation != generation {
                            return false;
                        }
                        let Some(namer) = &mut this.timbre_map.namer else {
                            return false;
                        };
                        let Some(result) = namer.poll() else {
                            return true;
                        };
                        this.timbre_map.finish_naming(generation, result);
                        cx.notify();
                        false
                    })
                    .unwrap_or(false);
                if !active {
                    break;
                }
            }
        })
        .detach();
    }

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
        self.timbre_map.hovered = None;
        self.timbre_map.error = None;
        self.timbre_map.names.clear();
        self.timbre_map.naming_error = None;
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

    pub(crate) fn render_timbre_map(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.timbre_map.open {
            return None;
        }
        let theme = &self.theme.atelier;
        let spacing = theme.spacing;
        let busy = self.timbre_map.control.is_some();
        let mut naming_toolbar = None;
        let mut content = div().flex().flex_col().flex_1().min_h_0().gap(spacing.md);
        if let Some(control) = &self.timbre_map.control {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap(spacing.sm)
                    .child(
                        SectionHeader::new(self.t(Key::AnalysisRunning))
                            .description(format!(
                                "{}/{}",
                                control.completed(),
                                self.timbre_map.total
                            ))
                            .action(Button::new("timbre-cancel", self.t(Key::Cancel)).on_click(
                                cx.listener(|this, _, _, cx| {
                                    this.timbre_map.cancel();
                                    cx.notify();
                                }),
                            )),
                    )
                    .child(ProgressBar::new(
                        control.completed() as f32 / self.timbre_map.total.max(1) as f32,
                    )),
            );
        }
        if let Some(error) = &self.timbre_map.error {
            content = content.child(
                Label::new(format!("! {error}"))
                    .color(TextColor::Danger)
                    .size(TextSize::Small),
            );
        }
        if let Some(map) = &self.timbre_map.map {
            if map.sounds.is_empty() {
                content = content.child(EmptyState::new(
                    IconName::Music,
                    self.t(Key::TimbreMapEmpty),
                ));
            } else {
                let (positions, extent) = plot_data(map.positions());
                let neighbors = self
                    .timbre_map
                    .selected
                    .map(|i| map.nearest(i, 8))
                    .unwrap_or_default();
                let mut plot = Plot3d::new()
                    .labels("PC1", "PC2", "PC3")
                    .x_range(-extent, extent)
                    .y_range(-extent, extent)
                    .z_range(-extent, extent)
                    .view(
                        self.timbre_map.camera.azimuth,
                        self.timbre_map.camera.elevation,
                    );
                for batch in
                    self.timbre_map
                        .camera
                        .scatter_batches(map.clusters(), &positions, extent)
                {
                    let cluster = map.clusters()[batch[0]];
                    let points = batch.into_iter().map(|i| positions[i]).collect();
                    plot = plot.scatter(points, Some(cluster_color(cluster, &self.theme)));
                }
                // Small three-axis crosses identify the selected source without relying on hue.
                if let Some(index) = self.timbre_map.selected {
                    for axis in 0..3 {
                        let mut a = positions[index];
                        let mut b = a;
                        a[axis] -= extent * 0.035;
                        b[axis] += extent * 0.035;
                        plot = plot.curve(vec![a, b], Some(theme.colors.text));
                    }
                    for (i, _) in &neighbors {
                        plot = plot.scatter(vec![positions[*i]], Some(theme.colors.accent));
                    }
                }
                let bounds = self.timbre_map.camera.bounds.clone();
                let picking = positions.clone();
                let hovering = positions;
                let mut well = div()
                    .id("timbre-plot")
                    .debug_selector(|| "timbre-plot".into())
                    .relative()
                    .flex_1()
                    .min_h(gpui::rems(12.))
                    .w_full()
                    .rounded(theme.shape.radius_sm)
                    .bg(theme.colors.sunken)
                    .overflow_hidden()
                    .cursor_grab()
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, event: &gpui::MouseDownEvent, _, cx| {
                            this.timbre_map.camera.press(event.position);
                            cx.stop_propagation();
                        }),
                    )
                    .on_mouse_move(
                        cx.listener(move |this, event: &gpui::MouseMoveEvent, _, cx| {
                            if this
                                .timbre_map
                                .camera
                                .drag(event.position, event.dragging())
                            {
                                this.timbre_map.hovered = None;
                                cx.notify();
                            } else {
                                let hovered =
                                    this.timbre_map
                                        .camera
                                        .pick(&hovering, extent, event.position);
                                if this.timbre_map.hovered != hovered {
                                    this.timbre_map.hovered = hovered;
                                    cx.notify();
                                }
                            }
                        }),
                    )
                    .on_mouse_up(
                        gpui::MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseUpEvent, _, cx| {
                            if this.timbre_map.camera.release()
                                && let Some(i) =
                                    this.timbre_map
                                        .camera
                                        .pick(&picking, extent, event.position)
                            {
                                this.select_timbre(i);
                                cx.notify();
                            }
                        }),
                    )
                    .on_mouse_up_out(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, _, _| {
                            this.timbre_map.camera.release();
                        }),
                    )
                    .child(plot)
                    .child(
                        gpui::canvas(
                            move |rect, _, _| {
                                bounds.set(rect);
                            },
                            |_, _, _, _| {},
                        )
                        .absolute()
                        .top_0()
                        .left_0()
                        .size_full(),
                    );
                if let Some(index) = self.timbre_map.hovered.filter(|i| *i < map.sounds.len()) {
                    well = well.tooltip(crate::ui::tooltip::keyed_tip(
                        format!(
                            "{} · {}",
                            map.sounds[index].name,
                            self.timbre_cluster_name(map.clusters()[index])
                        ),
                        "",
                        &self.theme,
                    ));
                }
                let camera = &self.timbre_map.camera;
                let views = SegmentedControl::new(
                    "timbre-views",
                    [
                        Key::TimbreMapView3d,
                        Key::TimbreMapViewTop,
                        Key::TimbreMapViewFront,
                        Key::TimbreMapViewSide,
                    ]
                    .map(|key| Segment::label(self.t(key))),
                    camera.preset.unwrap_or(usize::MAX),
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, index: &usize, _, cx| {
                    this.timbre_map.camera.set_view(*index);
                    cx.notify();
                }));
                let mut turns = div().flex().items_center().gap(spacing.xs);
                for (id, icon, label, azimuth, elevation) in [
                    (
                        "timbre-left",
                        IconName::ChevronLeft,
                        Key::TimbreMapTurnLeft,
                        -15.,
                        0.,
                    ),
                    (
                        "timbre-right",
                        IconName::ChevronRight,
                        Key::TimbreMapTurnRight,
                        15.,
                        0.,
                    ),
                    (
                        "timbre-up",
                        IconName::ChevronUp,
                        Key::TimbreMapTiltUp,
                        0.,
                        15.,
                    ),
                    (
                        "timbre-down",
                        IconName::ChevronDown,
                        Key::TimbreMapTiltDown,
                        0.,
                        -15.,
                    ),
                ] {
                    turns = turns.child(
                        Button::icon_only(id, icon)
                            .ghost()
                            .size(ControlSize::Small)
                            .tooltip(self.t(label))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.timbre_map.camera.turn(azimuth, elevation);
                                cx.notify();
                            })),
                    );
                }
                let mut legend = div()
                    .id("timbre-legend")
                    .flex()
                    .flex_wrap()
                    .gap(spacing.sm)
                    .max_h(gpui::rems(7.))
                    .overflow_y_scroll();
                for summary in map.cluster_summaries() {
                    let cluster = summary.id;
                    let name = self.timbre_cluster_name(cluster);
                    legend = legend.child(
                        div()
                            .id(SharedString::from(format!("timbre-cluster-{cluster}")))
                            .flex()
                            .items_center()
                            .gap(spacing.xs)
                            .max_w(gpui::rems(15.))
                            .tooltip(super::tooltip::keyed_tip(
                                format!("{name}\n{}", summary.examples.join(" · ")),
                                "",
                                &self.theme,
                            ))
                            .child(StatusDot::new(cluster_color(cluster, &self.theme)))
                            .child(
                                div().min_w_0().truncate().child(
                                    Label::new(format!("{name} · {}", summary.sound_count))
                                        .size(TextSize::XSmall)
                                        .color(TextColor::Muted),
                                ),
                            ),
                    );
                }
                let naming = self.timbre_map.namer.is_some();
                let mut naming_controls = div()
                    .flex()
                    .flex_1()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing.sm)
                    .child(
                        Button::new(
                            "timbre-name-clusters",
                            self.t(if naming {
                                Key::TimbreMapNaming
                            } else {
                                Key::TimbreMapNameWithAi
                            }),
                        )
                        .icon(IconName::Sparkles)
                        .variant(ButtonVariant::Agent)
                        .size(ControlSize::Small)
                        .disabled(naming || !self.settings.agent.is_configured())
                        .tooltip(self.t(if self.settings.agent.is_configured() {
                            Key::TimbreMapNamingHint
                        } else {
                            Key::TimbreMapConfigureAi
                        }))
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.name_timbre_clusters(cx);
                            cx.notify();
                        })),
                    );
                if naming {
                    naming_controls = naming_controls.child(
                        Button::new("timbre-cancel-naming", self.t(Key::Cancel))
                            .ghost()
                            .size(ControlSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timbre_map.cancel_naming();
                                cx.notify();
                            })),
                    );
                } else if !self.timbre_map.names.is_empty() {
                    naming_controls = naming_controls.child(
                        Button::new("timbre-reset-names", self.t(Key::TimbreMapResetNames))
                            .ghost()
                            .size(ControlSize::Small)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timbre_map.cancel_naming();
                                this.timbre_map.names.clear();
                                this.timbre_map.naming_error = None;
                                cx.notify();
                            })),
                    );
                }
                naming_toolbar = Some(naming_controls.into_any_element());
                let figure = Card::new()
                    .flex_1()
                    .min_w_0()
                    .min_h(gpui::rems(24.))
                    .p(spacing.lg)
                    .gap(spacing.sm)
                    .child(
                        SectionHeader::new(self.t(Key::TimbreMap))
                            .description(self.t(Key::TimbreMapHint)),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(spacing.sm)
                            .child(views)
                            .child(turns)
                            .child(
                                Label::new(format!(
                                    "{:.0}° / {:.0}°",
                                    camera.azimuth, camera.elevation
                                ))
                                .mono()
                                .size(TextSize::XSmall)
                                .color(TextColor::Muted),
                            ),
                    )
                    .child(well)
                    .child(
                        Label::new(self.t(Key::TimbreMapGeometry))
                            .size(TextSize::XSmall)
                            .color(TextColor::Muted),
                    );
                let selected = self.timbre_map.selected.and_then(|i| map.sounds.get(i));
                let target = self
                    .selected_track
                    .filter(|id| self.session.audition_track(Some(*id)) == Some(*id));
                let list_nearest = self.timbre_map.list_nearest;
                let entries: Vec<_> = if list_nearest {
                    neighbors
                        .iter()
                        .map(|(i, distance)| (*i, Some(*distance)))
                        .collect()
                } else {
                    (0..map.sounds.len()).map(|i| (i, None)).collect()
                };
                let list_id = if list_nearest {
                    "timbre-neighbors"
                } else {
                    "timbre-all"
                };
                let narrow = window.viewport_size().width < window.rem_size() * 48.;
                let inspector = div()
                    .flex()
                    .flex_col()
                    .gap(spacing.md)
                    .w(gpui::rems(20.))
                    .when(narrow, |column| {
                        column.w_full().h(gpui::rems(24.)).flex_shrink_0()
                    })
                    .max_w_full()
                    .min_w_0()
                    .min_h(gpui::rems(18.))
                    .child(SectionHeader::new(selected.map_or_else(
                        || self.t(Key::TimbreMapSelect).to_owned(),
                        |s| s.name.clone(),
                    )))
                    .children(
                        self.session
                            .audition_track(self.selected_track)
                            .and_then(|id| self.project().track(id))
                            .map(|track| {
                                Label::new(format!(
                                    "{}: {}",
                                    self.t(Key::TimbreMapAudition),
                                    track.name
                                ))
                                .size(TextSize::Small)
                                .color(TextColor::Muted)
                            }),
                    )
                    .child(
                        Button::new("timbre-use", self.t(Key::TimbreMapUse))
                            .primary()
                            .disabled(selected.is_none() || target.is_none())
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.adopt_timbre();
                                cx.notify();
                            })),
                    )
                    .child(
                        SegmentedControl::new(
                            "timbre-lists",
                            [
                                Segment::label(self.t(Key::TimbreMapAll)),
                                Segment::label(self.t(Key::TimbreMapNearest)),
                            ],
                            usize::from(list_nearest),
                        )
                        .size(ControlSize::Small)
                        .on_select(cx.listener(
                            |this, i: &usize, _, cx| {
                                this.timbre_map.list_nearest = *i == 1;
                                cx.notify();
                            },
                        )),
                    )
                    .child(
                        div()
                            .id(list_id)
                            .flex_1()
                            .min_h(gpui::rems(10.))
                            .p(spacing.xs)
                            .overflow_y_scroll()
                            .children(entries.into_iter().map(|(i, distance)| {
                                let mut row = ListItem::new(
                                    SharedString::from(format!("{list_id}-{i}")),
                                    map.sounds[i].name.clone(),
                                )
                                .selected(self.timbre_map.selected == Some(i))
                                .icon(IconName::Music)
                                .icon_color(cluster_color(map.clusters()[i], &self.theme))
                                .description(self.timbre_cluster_name(map.clusters()[i]))
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_timbre(i);
                                    cx.notify();
                                }));
                                if let Some(distance) = distance {
                                    row = row.trailing(
                                        Label::new(format!("{distance:.2}"))
                                            .mono()
                                            .size(TextSize::XSmall)
                                            .color(TextColor::Muted),
                                    );
                                }
                                row
                            })),
                    );
                content = content
                    .child(
                        div()
                            .id("timbre-map-body")
                            .flex()
                            .flex_1()
                            .min_h_0()
                            .gap(spacing.lg)
                            .overflow_y_scroll()
                            .when(narrow, |row| row.flex_col().overflow_y_scroll())
                            .child(figure)
                            .child(inspector),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .flex_shrink_0()
                            .gap(spacing.sm)
                            .child(legend)
                            .child(
                                Label::new(self.t(if self.timbre_map.names.is_empty() {
                                    Key::TimbreMapRepresentativeNames
                                } else {
                                    Key::TimbreMapAiNames
                                }))
                                .size(TextSize::XSmall)
                                .color(TextColor::Muted),
                            )
                            .children(self.timbre_map.naming_error.as_ref().map(|error| {
                                Label::new(format!(
                                    "{}: {error}",
                                    self.t(Key::TimbreMapNamingFailed)
                                ))
                                .size(TextSize::Small)
                                .color(TextColor::Danger)
                            })),
                    );
            }
        } else if !busy {
            content = content.child(EmptyState::new(
                IconName::Music,
                self.t(Key::TimbreMapReady),
            ));
        }
        let summary = self
            .timbre_map
            .map
            .as_ref()
            .map(|map| {
                format!(
                    "{}: {:.0}% · {}: {}",
                    self.t(Key::TimbreMapVariance),
                    map.explained_variance() * 100.,
                    self.t(Key::TimbreMapSkipped),
                    map.skipped.len()
                )
            })
            .unwrap_or_default();
        Some(
            div()
                .id("timbre-map")
                .key_context("AurisVisualizationControls")
                .on_action(|_: &super::widgets::ClaimVisualizationActivation, _, _| {})
                .size_full()
                .flex()
                .flex_col()
                .bg(theme.window_fill())
                .text_color(theme.colors.text)
                .font(theme.typography.ui.clone())
                .occlude()
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .justify_end()
                        .gap(spacing.sm)
                        .p(spacing.md)
                        .flex_shrink_0()
                        .border_b_1()
                        .border_color(theme.colors.separator)
                        .children(naming_toolbar)
                        .child(
                            Button::new("timbre-rescan", self.t(Key::TimbreMapRescan))
                                .size(ControlSize::Small)
                                .disabled(busy)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scan_timbre_map(cx);
                                    cx.notify();
                                })),
                        )
                        .child(
                            Button::new("timbre-close", self.t(Key::Close))
                                .ghost()
                                .size(ControlSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.close_timbre_map();
                                    cx.notify();
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .flex_1()
                        .min_h_0()
                        .p(spacing.lg)
                        .child(content),
                )
                .child(StatusBar::new().message(summary).busy(busy))
                .into_any_element(),
        )
    }
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
            // Use the first alternate sound: the visible rows remain clickable as the built-in
            // catalogue grows, without relying on FM2 staying above the scroll fold.
            let index = map
                .sounds
                .iter()
                .position(|s| s.instrument_id != auris_session::DEFAULT_INSTRUMENT)
                .unwrap();
            this.timbre_map.map = Some(Arc::new(map));
            this.timbre_map.open = true;
            let map = this.timbre_map.map.as_ref().unwrap();
            let cluster = map.clusters()[index];
            let sound = &map.sounds[index];
            let annotation = this
                .library_timbre_cluster(&sound.instrument_id, sound.preset)
                .unwrap();
            assert_eq!(annotation.0, cluster_color(cluster, &this.theme));
            assert!(!annotation.1.is_empty());
            assert!(this.library_timbre_cluster("unmeasured", None).is_none());
            (this.project().clone(), index)
        });
        paint(&app, cx);
        // GPUI's debug selector API requires a static string, including for dynamic rows.
        let mut utility = map_window(&app, cx);
        // Picking rotates the display only; it must never author instrument changes.
        let (picked, at) = app.read_with(&utility, |this, _| {
            let map = this.timbre_map.map.as_ref().unwrap();
            let (positions, extent) = plot_data(map.positions());
            let camera = &this.timbre_map.camera;
            camera
                .projected(&positions, extent)
                .iter()
                .enumerate()
                .find_map(|(i, (at, _))| {
                    (camera.pick(&positions, extent, *at) == Some(i)).then_some((i, *at))
                })
                .unwrap()
        });
        utility.simulate_click(at, gpui::Modifiers::none());
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.selected, Some(picked));
            assert_eq!(this.project(), &before);
        });
        let center = utility.debug_bounds("timbre-plot").unwrap().center();
        crate::harness::drag(
            &mut utility,
            center,
            center + gpui::point(gpui::px(50.), gpui::px(20.)),
        );
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.selected, Some(picked));
            assert_eq!(this.timbre_map.camera.preset, None);
            assert_eq!(this.project(), &before);
        });
        // View presets and turn buttons remain reachable without pointer gestures.
        paint(&app, &mut utility);
        click("timbre-views-1", &mut utility);
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.camera.elevation, 90.)
        });
        click("timbre-right", &mut utility);
        for configured in [false, true] {
            // Other tests can persist a model into this run's shared preferences. Set the
            // condition explicitly and cover both Tab paths: enabled AI naming adds one stop.
            app.update(&mut utility, |this, _| {
                this.settings.agent = Default::default();
                if configured {
                    this.settings.agent.model = "offline-fixture".into();
                }
            });
            paint(&app, &mut utility);
            let azimuth = app.read_with(&utility, |this, _| this.timbre_map.camera.azimuth);
            utility.update(|window, cx| window.focus(&app.read(cx).focus));
            // AI naming when enabled, Rescan, Close, four standard views, then left and right.
            utility.simulate_keystrokes(if configured {
                "tab tab tab tab tab tab tab tab tab"
            } else {
                "tab tab tab tab tab tab tab tab"
            });
            utility.simulate_keystrokes("space");
            utility.simulate_event(gpui::KeyUpEvent {
                keystroke: gpui::Keystroke::parse("space").unwrap(),
            });
            app.read_with(&utility, |this, _| {
                assert_eq!(this.timbre_map.camera.azimuth, azimuth + 15.)
            });
        }
        paint(&app, &mut utility);
        click(
            Box::leak(format!("timbre-all-{index}").into_boxed_str()),
            &mut utility,
        );
        app.read_with(cx, |this, _| {
            assert_eq!(this.project(), &before);
            assert_eq!(this.timbre_map.selected, Some(index));
        });
        for direction in [
            atelier::Direction::Studio,
            atelier::Direction::Glass,
            atelier::Direction::Mono,
        ] {
            for appearance in [atelier::Appearance::Light, atelier::Appearance::Dark] {
                app.update(&mut utility, |this, cx| {
                    this.theme.atelier = atelier::Theme::new(direction, appearance);
                    this.theme.install(cx);
                    cx.notify();
                });
                crate::harness::resize(
                    &app,
                    &mut utility,
                    gpui::size(gpui::px(900.), gpui::px(700.)),
                );
                assert!(utility.debug_bounds("timbre-plot").is_some());
                assert!(
                    utility
                        .debug_bounds("timbre-name-clusters")
                        .unwrap()
                        .bottom()
                        < gpui::px(700.)
                );
                crate::harness::resize(
                    &app,
                    &mut utility,
                    gpui::size(gpui::px(640.), gpui::px(540.)),
                );
                assert!(utility.debug_bounds("timbre-plot").is_some());
                assert!(
                    utility
                        .debug_bounds("timbre-name-clusters")
                        .unwrap()
                        .bottom()
                        < gpui::px(540.)
                );
            }
        }
        crate::harness::resize(
            &app,
            &mut utility,
            gpui::size(gpui::px(900.), gpui::px(700.)),
        );
        app.update(&mut utility, |this, _| {
            let map = this.timbre_map.map.as_ref().unwrap();
            let names = map
                .cluster_summaries()
                .iter()
                .map(|cluster| ClusterName {
                    id: cluster.id,
                    name: format!("Named cluster {}", cluster.id),
                })
                .collect();
            let sound = map.sounds[index].clone();
            let cluster = map.clusters()[index];
            let generation = this.timbre_map.naming_generation;
            assert!(this.timbre_map.finish_naming(generation, Ok(names)));
            assert_eq!(
                this.library_timbre_cluster(&sound.instrument_id, sound.preset)
                    .unwrap()
                    .1,
                format!("Named cluster {cluster}")
            );
            assert_eq!(this.project(), &before);
        });
        paint(&app, &mut utility);
        click("timbre-reset-names", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(this.timbre_map.names.is_empty());
            assert_eq!(this.project(), &before);
        });
        paint(&app, &mut utility);
        click("timbre-use", &mut utility);
        app.update(cx, |this, _| {
            assert_ne!(this.project(), &before);
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
        click("timbre-close", &mut utility);
        app.update(cx, |this, _| {
            assert!(!this.timbre_map.open);
            let obsolete = this.timbre_map.naming_generation;
            this.timbre_map.cancel_naming();
            assert!(!this.timbre_map.finish_naming(
                obsolete,
                Ok(vec![ClusterName {
                    id: 0,
                    name: "Obsolete".into()
                }])
            ));
            assert!(this.timbre_map.names.is_empty());
            let map = this.timbre_map.map.as_ref().unwrap();
            let sound = &map.sounds[index];
            assert!(
                this.library_timbre_cluster(&sound.instrument_id, sound.preset)
                    .is_some()
            );
            this.reset_view();
            assert!(
                this.library_timbre_cluster("auris.physical.piano", None)
                    .is_none()
            );
        });
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
