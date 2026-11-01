//! Library timbre exploration with a background session job and explicit adoption.

use crate::app::AurisApp;
use atelier::plot::{LinePlot, Plot3d, PlotPoint, Series};
use atelier::{
    Button, ButtonVariant, Card, ControlSize, Disclosure, EmptyState, IconName, Label, ListItem,
    ProgressBar, SectionHeader, Segment, SegmentedControl, StatusBar, TextColor, TextSize,
};
use auris_agent::timbre_names::{ClusterName, TimbreNamer};
use auris_i18n::Key;
use auris_session::{
    SoundIcon, TimbreMap, TimbreMapControl, TimbreSound, TimbreTargets, prelude::*,
};
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};
use std::sync::Arc;
#[path = "timbre_plot.rs"]
mod plot;
use plot::{TimbreView, plot_data};
#[path = "timbre_envelope.rs"]
mod envelope;
pub(crate) use envelope::EnvelopeFilter;
use envelope::Stage;

/// A library row's note of the timbre cluster its sound belongs to.
pub(crate) struct TimbreClusterTag {
    /// The cluster's colour, the same in the map, its legend and the library.
    pub color: gpui::Hsla,
    /// The cluster's name.
    pub name: String,
    /// Whether "Name with AI" wrote the name, so it carries the agent's mark.
    pub by_ai: bool,
}

/// A cluster's name in small muted text. The agent's mark follows it when the agent wrote it,
/// so an AI name reads as one wherever it appears: legend, list or library. The rows that show
/// it carry their own tooltip, which says who named it, so the label takes none of its own.
pub(crate) fn cluster_label(name: impl Into<SharedString>, by_ai: bool) -> AnyElement {
    let name = name.into();
    if by_ai {
        atelier::agent::AgentLabel::new(name)
            .size(TextSize::XSmall)
            .color(TextColor::Muted)
            .into_any_element()
    } else {
        Label::new(name)
            .size(TextSize::XSmall)
            .color(TextColor::Muted)
            .into_any_element()
    }
}

#[derive(Default)]
pub(crate) struct TimbreMapState {
    pub(crate) open: bool,
    camera: TimbreView,
    hovered: Option<usize>,
    /// What the map's three axes are: 0 similarity, 1 sound character, 2 measured envelope.
    arrangement: usize,
    /// The sound whose nearest neighbours the list is narrowed to, until the chip is removed.
    similar_to: Option<usize>,
    /// Envelope ranges the list is ranked by.
    pub(crate) envelope: EnvelopeFilter,
    envelope_open: bool,
    pub(crate) search: super::text_field::TextField,
    pub(crate) search_focused: bool,
    pub(crate) map: Option<Arc<TimbreMap>>,
    pub(crate) control: Option<TimbreMapControl>,
    generation: u64,
    total: usize,
    selected: Option<usize>,
    preview: TimbrePreview,
    error: Option<String>,
    names: Vec<ClusterName>,
    namer: Option<TimbreNamer>,
    naming_generation: u64,
    naming_error: Option<String>,
    editing_icon: bool,
}

#[derive(Default)]
struct TimbrePreview {
    serial: u64,
    control: Option<TimbreMapControl>,
    status: Option<auris_session::OutputPreviewStatus>,
    pitch: Option<u8>,
}

impl TimbrePreview {
    fn stop(&mut self) {
        self.serial = self.serial.wrapping_add(1);
        if let Some(control) = self.control.take() {
            control.cancel();
        }
        if let Some(status) = self.status.take() {
            status.stop();
        }
    }
}

impl Drop for TimbrePreview {
    fn drop(&mut self) {
        self.stop();
    }
}

impl TimbreMapState {
    pub(crate) fn clear(&mut self) {
        self.cancel();
        self.map = None;
        self.names.clear();
        self.naming_error = None;
        self.editing_icon = false;
        self.search = Default::default();
        self.search_focused = false;
        self.similar_to = None;
        self.envelope = EnvelopeFilter::default();
    }

    pub(crate) fn cancel(&mut self) {
        self.preview.stop();
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

fn pick_source_index(series: &[Vec<usize>], point: Option<PlotPoint>) -> Option<usize> {
    let point = point?;
    series.get(point.series)?.get(point.index).copied()
}

fn sound_icon_label(icon: SoundIcon) -> Key {
    match icon {
        SoundIcon::Sound => Key::SoundIconSound,
        SoundIcon::Piano => Key::SoundIconPiano,
        SoundIcon::Organ => Key::SoundIconOrgan,
        SoundIcon::Guitar => Key::SoundIconGuitar,
        SoundIcon::Bass => Key::SoundIconBass,
        SoundIcon::Strings => Key::SoundIconStrings,
        SoundIcon::Brass => Key::SoundIconBrass,
        SoundIcon::Wind => Key::SoundIconWind,
        SoundIcon::Drum => Key::SoundIconDrum,
        SoundIcon::Bell => Key::SoundIconBell,
        SoundIcon::Voice => Key::SoundIconVoice,
        SoundIcon::Synth => Key::SoundIconSynth,
        SoundIcon::Pad => Key::SoundIconPad,
        SoundIcon::Pluck => Key::SoundIconPluck,
    }
}

impl AurisApp {
    fn render_timbre_search(&self, cx: &mut Context<Self>) -> AnyElement {
        let state = &self.timbre_map;
        let theme = self.theme.clone();
        let field = div()
            .relative()
            .flex_1()
            .min_w_0()
            .h_full()
            .child(if state.search_focused {
                super::prompt::editable_text(
                    state.search.content().to_owned().into(),
                    state.search.selection(),
                    state.search.marked(),
                    self.focus.clone(),
                    cx.entity(),
                    theme.clone(),
                )
                .into_any_element()
            } else {
                super::prompt::field_text(state.search.content().to_owned(), theme.text)
                    .into_any_element()
            })
            .when(state.search.content().is_empty(), |field| {
                field.child(
                    super::prompt::field_text(
                        self.t(Key::TimbreSearchPlaceholder),
                        theme.text_muted,
                    )
                    .absolute()
                    .inset_0(),
                )
            });
        div()
            .flex()
            .items_center()
            .gap(theme.atelier.spacing.xs)
            .child(
                Button::icon_only("timbre-search-focus", IconName::Search)
                    .ghost()
                    .size(ControlSize::Small)
                    .tooltip(self.t(Key::TimbreSearch))
                    .on_click(cx.listener(|this, _, window, cx| {
                        this.agent_chat.focused = None;
                        this.library_search_focused = false;
                        this.timbre_map.search_focused = true;
                        window.focus(&this.focus);
                        cx.notify();
                    })),
            )
            .child(
                div()
                    .id("timbre-search-field")
                    .debug_selector(|| "timbre-search-field".into())
                    .flex()
                    .flex_1()
                    .min_w_0()
                    .h_6()
                    .rounded(theme.atelier.shape.radius_sm)
                    .bg(theme.atelier.colors.sunken)
                    .border_1()
                    .border_color(if state.search_focused {
                        theme.atelier.colors.accent
                    } else {
                        theme.border_subtle
                    })
                    .cursor_text()
                    .child(field)
                    .on_mouse_down(
                        gpui::MouseButton::Left,
                        cx.listener(|this, _, window, cx| {
                            this.agent_chat.focused = None;
                            this.library_search_focused = false;
                            this.timbre_map.search_focused = true;
                            window.focus(&this.focus);
                            cx.notify();
                        }),
                    ),
            )
            .when(!state.search.content().is_empty(), |row| {
                row.child(
                    Button::new("timbre-search-clear", self.t(Key::TimbreSearchClear))
                        .ghost()
                        .size(ControlSize::Small)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.timbre_map.search = Default::default();
                            this.timbre_map.search_focused = false;
                            cx.notify();
                        })),
                )
            })
            .into_any_element()
    }

    pub(crate) fn timbre_search_key(
        &mut self,
        event: &gpui::KeyDownEvent,
        cx: &mut Context<Self>,
    ) -> bool {
        let key = event.keystroke.key.as_str();
        if self.timbre_map.search.marked().is_none() && matches!(key, "escape" | "enter" | "tab") {
            self.timbre_map.search_focused = false;
            cx.notify();
            return key != "tab";
        }
        self.timbre_map.search.apply_key_with_clipboard(
            key,
            event.keystroke.modifiers.shift,
            event.keystroke.modifiers.secondary(),
            false,
            cx,
        ) != super::text_field::KeyEffect::Ignored
    }

    /// Manual corrections win over name hints, then cluster suggestions fill unknown families.
    pub(crate) fn library_sound_icon(
        &self,
        instrument_id: &str,
        preset: Option<PresetRef>,
        name: &str,
    ) -> SoundIcon {
        if let Some(icon) = self
            .session
            .library_sound_key(instrument_id, preset)
            .and_then(|key| self.settings.sound_icons.get(&key))
        {
            return *icon;
        }
        let inferred = SoundIcon::infer(name);
        if inferred != SoundIcon::Sound {
            return inferred;
        }
        let Some(map) = &self.timbre_map.map else {
            return SoundIcon::Sound;
        };
        let Some(cluster) = map.cluster_for(instrument_id, preset) else {
            return SoundIcon::Sound;
        };
        self.timbre_cluster_icon(cluster)
    }

    fn timbre_cluster_icon(&self, cluster: usize) -> SoundIcon {
        self.timbre_map
            .names
            .iter()
            .find(|name| name.id == cluster)
            .map(|name| name.icon)
            .or_else(|| {
                self.timbre_map
                    .map
                    .as_ref()?
                    .cluster_summaries()
                    .iter()
                    .find(|summary| summary.id == cluster)
                    .map(|summary| summary.icon())
            })
            .unwrap_or_default()
    }

    fn correct_sound_icon_with<E: std::fmt::Display>(
        &mut self,
        sound: &TimbreSound,
        icon: Option<SoundIcon>,
        save: impl FnOnce(&auris_session::Settings) -> Result<(), E>,
    ) {
        let Some(key) = self
            .session
            .library_sound_key(&sound.instrument_id, sound.preset)
        else {
            return;
        };
        let previous = self.settings.sound_icons.clone();
        if let Some(icon) = icon {
            self.settings.sound_icons.insert(key, icon);
        } else {
            self.settings.sound_icons.remove(&key);
        }
        match save(&self.settings) {
            Ok(()) => self.timbre_map.error = None,
            Err(error) => {
                self.settings.sound_icons = previous;
                self.timbre_map.error = Some(error.to_string());
            }
        }
    }

    fn correct_selected_sound_icon(&mut self, icon: Option<SoundIcon>) {
        let sound = self
            .timbre_map
            .map
            .as_ref()
            .and_then(|map| self.timbre_map.selected.and_then(|i| map.sounds.get(i)))
            .cloned();
        if let Some(sound) = sound {
            self.correct_sound_icon_with(&sound, icon, self.settings_saver());
        }
    }

    fn render_sound_icon_picker(&self, sound: &TimbreSound, cx: &mut Context<Self>) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let selected = self.library_sound_icon(&sound.instrument_id, sound.preset, &sound.name);
        let manual = self
            .session
            .library_sound_key(&sound.instrument_id, sound.preset)
            .is_some_and(|key| self.settings.sound_icons.contains_key(&key));
        let mut picker = div().flex().flex_col().gap(spacing.xs).child(
            Button::new(
                "timbre-edit-icon",
                format!(
                    "{} · {}",
                    self.t(sound_icon_label(selected)),
                    self.t(Key::TimbreMapEditIcon)
                ),
            )
            .icon(atelier::IconSource::svg(super::icons::sound_icon_path(
                selected,
            )))
            .size(ControlSize::Small)
            .on_click(cx.listener(|this, _, _, cx| {
                this.timbre_map.editing_icon = !this.timbre_map.editing_icon;
                cx.notify();
            })),
        );
        if self.timbre_map.editing_icon {
            picker = picker
                .child(
                    Label::new(self.t(Key::TimbreMapIconHint))
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                )
                .child(
                    div()
                        .id("timbre-icon-presets")
                        .flex()
                        .flex_wrap()
                        .gap(spacing.xs)
                        .max_h(gpui::rems(7.))
                        .overflow_y_scroll()
                        .children(SoundIcon::ALL.iter().copied().map(|family| {
                            Button::new(
                                SharedString::from(format!("timbre-icon-{}", family.id())),
                                self.t(sound_icon_label(family)),
                            )
                            .icon(atelier::IconSource::svg(super::icons::sound_icon_path(
                                family,
                            )))
                            .ghost()
                            .selected(selected == family)
                            .size(ControlSize::Small)
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.correct_selected_sound_icon(Some(family));
                                    cx.notify();
                                },
                            ))
                        })),
                )
                .child(
                    Button::new("timbre-icon-auto", self.t(Key::TimbreMapAutoIcon))
                        .ghost()
                        .size(ControlSize::Small)
                        .disabled(!manual)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.correct_selected_sound_icon(None);
                            cx.notify();
                        })),
                );
        }
        picker.into_any_element()
    }

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

    /// Whether the cluster's name is one "Name with AI" wrote, which is drawn with the agent's
    /// mark wherever it appears. The fallback, a representative sound's name, is nobody's.
    fn timbre_cluster_named_by_ai(&self, cluster: usize) -> bool {
        self.timbre_map.names.iter().any(|name| name.id == cluster)
    }

    /// Cluster annotation for an exact measured library choice, shared by tree and search rows.
    pub(crate) fn library_timbre_cluster(
        &self,
        instrument_id: &str,
        preset: Option<PresetRef>,
    ) -> Option<TimbreClusterTag> {
        let cluster = self
            .timbre_map
            .map
            .as_ref()?
            .cluster_for(instrument_id, preset)?;
        Some(TimbreClusterTag {
            color: self.theme.atelier.categorical(cluster),
            name: self.timbre_cluster_name(cluster),
            by_ai: self.timbre_cluster_named_by_ai(cluster),
        })
    }

    fn name_timbre_clusters(&mut self, cx: &mut Context<Self>) {
        let Some(map) = &self.timbre_map.map else {
            return;
        };
        if self.timbre_map.namer.is_some() || self.timbre_map.control.is_some() {
            return;
        }
        let summaries = map.cluster_summaries_with_icons(|sound| {
            self.session
                .library_sound_key(&sound.instrument_id, sound.preset)
                .and_then(|key| self.settings.sound_icons.get(&key))
                .copied()
                .unwrap_or_else(|| SoundIcon::infer(&sound.name))
        });
        let result = TimbreNamer::start(
            self.settings.agent.clone(),
            &summaries,
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

    /// Opens the finder, scanning the library the first time.
    pub(crate) fn open_timbre_map(&mut self, cx: &mut Context<Self>) {
        self.timbre_map.open = true;
        if self.timbre_map.map.is_none() && self.timbre_map.control.is_none() {
            self.scan_timbre_map(cx);
        }
    }

    pub(crate) fn close_timbre_map(&mut self) {
        self.timbre_map.search_focused = false;
        self.timbre_map.cancel();
        self.timbre_map.open = false;
    }

    pub(crate) fn scan_timbre_map(&mut self, cx: &mut Context<Self>) {
        self.timbre_map.search_focused = false;
        self.timbre_map.cancel();
        self.timbre_map.selected = None;
        self.timbre_map.similar_to = None;
        self.timbre_map.editing_icon = false;
        self.timbre_map.hovered = None;
        self.timbre_map.error = None;
        self.timbre_map.names.clear();
        self.timbre_map.naming_error = None;
        self.timbre_map.preview.pitch = None;
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

    fn select_timbre(&mut self, index: usize, cx: &mut Context<Self>) {
        self.timbre_map.search_focused = false;
        let Some(map) = self.timbre_map.map.clone() else {
            return;
        };
        if index >= map.sounds.len() {
            return;
        }
        self.timbre_map.selected = Some(index);
        self.timbre_map.editing_icon = false;
        self.timbre_map.error = None;
        self.preview_timbre_note(60, cx);
    }

    fn preview_timbre_note(&mut self, pitch: u8, cx: &mut Context<Self>) {
        self.timbre_map.preview.stop();
        if !self.acquire_audio(cx) {
            return;
        }
        let Some((map, index)) = self.timbre_map.map.as_ref().zip(self.timbre_map.selected) else {
            return;
        };
        let job = match self.session.timbre_preview_job(map, index, pitch) {
            Ok(job) => job,
            Err(error) => {
                self.timbre_map.error = Some(error.to_string());
                return;
            }
        };
        let control = TimbreMapControl::default();
        self.timbre_map.preview.control = Some(control.clone());
        self.timbre_map.preview.pitch = Some(pitch);
        self.timbre_map.error = None;
        let serial = self.timbre_map.preview.serial;
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { job.run(&control) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.timbre_map.preview.serial != serial || !this.timbre_map.open {
                    return;
                }
                this.timbre_map.preview.control = None;
                if this.session.audio_suspended() {
                    cx.notify();
                    return;
                }
                match result.and_then(|audio| {
                    if this.session.is_recording() {
                        return Err(auris_session::SessionError::RecordingInProgress);
                    }
                    this.session.play_output_preview(audio)
                }) {
                    Ok(status) => this.timbre_map.preview.status = Some(status),
                    Err(error) => this.timbre_map.error = Some(error.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
    }

    fn render_timbre_keyboard(&self, cx: &mut Context<Self>) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let enabled = self.timbre_map.selected.is_some();
        let keyboard = atelier::PianoKeyboard::octave("timbre-key")
            .pressed(self.timbre_map.preview.pitch)
            .disabled(!enabled)
            .on_press(cx.listener(|this, pitch: &u8, _, cx| {
                this.preview_timbre_note(*pitch, cx);
                cx.notify();
            }));
        div()
            .flex()
            .flex_col()
            .gap(spacing.sm)
            .flex_shrink_0()
            .child(Label::new(self.t(Key::TimbreMapKeyboard)).size(TextSize::Small))
            .child(keyboard)
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing.sm)
                    .child(
                        Button::new("timbre-replay", self.t(Key::TimbreMapReplay))
                            .size(ControlSize::Small)
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.preview_timbre_note(60, cx);
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("timbre-stop", self.t(Key::TimbreMapStop))
                            .size(ControlSize::Small)
                            .ghost()
                            .disabled(!enabled)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timbre_map.preview.stop();
                                cx.notify();
                            })),
                    )
                    .when(self.timbre_map.preview.control.is_some(), |row| {
                        row.child(
                            Label::new(self.t(Key::TimbreMapPreparing))
                                .size(TextSize::XSmall)
                                .color(TextColor::Muted),
                        )
                    }),
            )
            .into_any_element()
    }

    fn attach_timbre_to_chat(&mut self) {
        let Some(sound) = self
            .timbre_map
            .map
            .as_ref()
            .and_then(|map| self.timbre_map.selected.and_then(|i| map.sounds.get(i)))
            .cloned()
        else {
            return;
        };
        let value = auris_session::music_reference::sound_reference(&self.session, &sound);
        match self.attach_agent_music_reference(&value, &format!("sound-{}", sound.name)) {
            Ok(()) => {
                self.close_timbre_map();
                if self.song_workshop.is_none() {
                    self.panels.show(crate::dock::Panel::Agent);
                }
            }
            Err(error) => self.timbre_map.error = Some(error),
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

    /// The list the finder shows: every sound, or the chosen sound's neighbours, ranked by the
    /// search field and the envelope ranges when either says something. `None` ranks nothing.
    ///
    /// The field takes names and impressions together: a recognised impression ranks, every
    /// other word must occur in the sound's name. That is the same split the agent's
    /// `search_instruments` makes between its `query` and `name`.
    fn timbre_results(&self, map: &TimbreMap) -> (Option<Vec<(usize, f64)>>, String) {
        let query = self.timbre_map.search.content().trim();
        let envelope = self.timbre_map.envelope.targets();
        let candidates: Vec<usize> = match self.timbre_map.similar_to {
            Some(anchor) if anchor < map.sounds.len() => map
                .nearest(anchor, 12)
                .into_iter()
                .map(|(i, _)| i)
                .collect(),
            _ => (0..map.sounds.len()).collect(),
        };
        if query.is_empty() && envelope.is_empty() {
            return (None, self.t(Key::TimbreSearchHint).to_owned());
        }
        let (targets, recognized) =
            match TimbreTargets::interpret_partial(query, &TimbreTargets::default()) {
                Ok(found) => found,
                Err(_) => {
                    return (
                        Some(Vec::new()),
                        self.t(Key::TimbreSearchUnsupported).to_owned(),
                    );
                }
            };
        let lowered = query.to_lowercase();
        let names: Vec<&str> = lowered
            .split_whitespace()
            .filter(|word| !recognized.iter().any(|term| word.contains(term.as_str())))
            .collect();
        let ranked = map.search_character(&targets, &envelope);
        let results = ranked
            .into_iter()
            .filter(|(i, _)| candidates.contains(i))
            .filter(|(i, _)| {
                let name = map.sounds[*i].name.to_lowercase();
                names.iter().all(|word| name.contains(word))
            })
            .collect();
        let message = if recognized.is_empty() {
            self.t(Key::TimbreSearchHint).to_owned()
        } else {
            format!(
                "{}: {}",
                self.t(Key::TimbreSearchInterpreted),
                recognized.join(" · ")
            )
        };
        (Some(results), message)
    }

    /// The envelope ranges: a disclosure holding one choice per stage.
    fn render_envelope_filter(&self, cx: &mut Context<Self>) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let open = self.timbre_map.envelope_open;
        let mut column = div().flex().flex_col().gap(spacing.xs).child(
            Disclosure::new(
                "timbre-envelope-filter",
                self.t(Key::TimbreEnvelopeFilter),
                open,
            )
            .on_toggle(cx.listener(|this, _, _, cx| {
                this.timbre_map.envelope_open = !this.timbre_map.envelope_open;
                cx.notify();
            })),
        );
        if open {
            for stage in Stage::ALL {
                let choices = stage.choices();
                column = column.child(
                    div()
                        .flex()
                        .flex_col()
                        .gap(spacing.xs)
                        .child(
                            Label::new(self.t(stage.label()))
                                .size(TextSize::XSmall)
                                .color(TextColor::Muted),
                        )
                        .child(
                            SegmentedControl::new(
                                SharedString::from(format!("timbre-envelope-{stage:?}")),
                                [
                                    Segment::label(self.t(Key::EnvelopeAny)),
                                    Segment::label(self.t(choices[0])),
                                    Segment::label(self.t(choices[1])),
                                    Segment::label(self.t(choices[2])),
                                ],
                                self.timbre_map.envelope.get(stage),
                            )
                            .size(ControlSize::Small)
                            .on_select(cx.listener(
                                move |this, choice: &usize, _, cx| {
                                    this.timbre_map.envelope.set(stage, *choice);
                                    cx.notify();
                                },
                            )),
                        ),
                );
            }
        }
        column.into_any_element()
    }

    /// The conditions in force that are not typed into the field, each removable on its own.
    fn render_timbre_conditions(&self, map: &TimbreMap, cx: &mut Context<Self>) -> AnyElement {
        let spacing = self.theme.atelier.spacing;
        let mut chips = div().flex().flex_wrap().gap(spacing.xs);
        if let Some(anchor) = self.timbre_map.similar_to.and_then(|i| map.sounds.get(i)) {
            chips = chips.child(
                Button::new(
                    "timbre-similar-chip",
                    format!("{}: {}", self.t(Key::TimbreMapNearest), anchor.name),
                )
                .icon_after(IconName::Close)
                .size(ControlSize::Small)
                .tooltip(self.t(Key::TimbreRemoveCondition))
                .on_click(cx.listener(|this, _, _, cx| {
                    this.timbre_map.similar_to = None;
                    cx.notify();
                })),
            );
        }
        for (stage, choice) in self.timbre_map.envelope.active() {
            chips = chips.child(
                Button::new(
                    SharedString::from(format!("timbre-envelope-chip-{stage:?}")),
                    format!("{}: {}", self.t(stage.label()), self.t(choice)),
                )
                .icon_after(IconName::Close)
                .size(ControlSize::Small)
                .tooltip(self.t(Key::TimbreRemoveCondition))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.timbre_map.envelope.set(stage, 0);
                    cx.notify();
                })),
            );
        }
        chips.into_any_element()
    }

    /// The selected sound's measured envelope: one line per pitch, the median drawn over them,
    /// and the four numbers.
    fn render_timbre_envelope(&self, map: &TimbreMap, index: usize) -> AnyElement {
        let theme = &self.theme.atelier;
        let spacing = theme.spacing;
        let Some(measured) = map.envelopes().get(index).and_then(Option::as_ref) else {
            return Label::new(self.t(Key::TimbreEnvelopeMissing))
                .size(TextSize::XSmall)
                .color(TextColor::Muted)
                .into_any_element();
        };
        let span = envelope::span(measured);
        let mut plot = LinePlot::new()
            .x_range(0., span as f32)
            .y_range(envelope::FLOOR_DB, 0.)
            .x_label("s")
            .y_label("dB")
            .grid(true);
        for (pitch, e) in &measured.by_pitch {
            plot = plot.series(
                Series::line(envelope::curve(e, span))
                    .name(envelope::pitch_name(*pitch))
                    .color(theme.colors.text_muted)
                    .faint(),
            );
        }
        plot = plot.series(
            Series::line(envelope::curve(&measured.summary, span)).color(theme.colors.accent),
        );
        let mut column = div()
            .id("timbre-envelope")
            .debug_selector(|| "timbre-envelope".into())
            .flex()
            .flex_col()
            .gap(spacing.xs)
            .child(div().h(gpui::rems(7.)).w_full().child(plot))
            .child(
                Label::new(envelope::summary(&measured.summary))
                    .mono()
                    .size(TextSize::Small),
            );
        for (pitch, e) in &measured.by_pitch {
            column = column.child(
                Label::new(format!(
                    "{} {}",
                    envelope::pitch_name(*pitch),
                    envelope::summary(e)
                ))
                .mono()
                .size(TextSize::XSmall)
                .color(TextColor::Muted),
            );
        }
        if measured.summary.decaying {
            column = column.child(
                Label::new(self.t(Key::TimbreEnvelopeDecaying))
                    .size(TextSize::XSmall)
                    .color(TextColor::Muted),
            );
        }
        column
            .child(
                Label::new(self.t(Key::TimbreEnvelopeMeasured))
                    .size(TextSize::XSmall)
                    .color(TextColor::Muted),
            )
            .into_any_element()
    }

    /// The sound finder: the map, the list it searches, and the sound chosen from either.
    ///
    /// Three columns with three jobs. The map is the canvas and comes first; the list beside it
    /// is where searching happens — one field for names and impressions, envelope ranges under
    /// it, and every condition in force shown as something that can be taken off; the inspector
    /// is everything about one sound: how it sounds, its measured envelope, and what can be
    /// done with it. A narrower window stacks them in the same order.
    pub(crate) fn render_timbre_map(
        &self,
        window: &gpui::Window,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.timbre_map.open {
            return None;
        }
        let search_field = self.render_timbre_search(cx);
        let theme = &self.theme.atelier;
        let spacing = theme.spacing;
        let busy = self.timbre_map.control.is_some();
        let mut content = div().flex().flex_col().flex_1().min_h_0().gap(spacing.md);
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
                let arrangement = self.timbre_map.arrangement;
                let arranged = match arrangement {
                    1 => Some(map.character_positions()),
                    2 => Some(map.envelope_positions()),
                    _ => None,
                };
                let (positions, extent) =
                    plot_data(arranged.as_deref().unwrap_or_else(|| map.positions()));
                let (search_results, search_message) = self.timbre_results(map);
                let neighbors = self
                    .timbre_map
                    .selected
                    .map(|i| map.nearest(i, 8))
                    .unwrap_or_default();
                let cluster_ids: Vec<usize> = {
                    let mut ids = map.clusters().to_vec();
                    ids.sort_unstable();
                    ids.dedup();
                    ids
                };
                let [x_label, y_label, z_label] = match arrangement {
                    1 => [
                        Key::TimbreAxisBrightness,
                        Key::TimbreAxisAttack,
                        Key::TimbreAxisSustain,
                    ],
                    2 => [
                        Key::TimbreAxisAttackTime,
                        Key::TimbreAxisSustainLevel,
                        Key::TimbreAxisReleaseTime,
                    ],
                    _ => [Key::TimbreAxisPc1, Key::TimbreAxisPc2, Key::TimbreAxisPc3],
                };
                let mut plot = Plot3d::new()
                    .id("timbre-plot-content")
                    .labels(self.t(x_label), self.t(y_label), self.t(z_label))
                    .x_range(-extent, extent)
                    .y_range(-extent, extent)
                    .z_range(-extent, extent)
                    .view(
                        self.timbre_map.camera.azimuth,
                        self.timbre_map.camera.elevation,
                    );
                // Plot3d numbers every scatter and curve call as a series. Keep the source
                // indices alongside those calls so picking a highlight cannot be mistaken for
                // an out-of-range cluster series.
                let mut pick_sources: Vec<Vec<usize>> = Vec::new();
                for cluster in cluster_ids.iter().copied() {
                    let source_indices: Vec<usize> = map
                        .clusters()
                        .iter()
                        .enumerate()
                        .filter_map(|(i, value)| (*value == cluster).then_some(i))
                        .collect();
                    let points = source_indices.iter().map(|i| positions[*i]).collect();
                    plot = plot.scatter(points, Some(self.theme.atelier.categorical(cluster)));
                    pick_sources.push(source_indices);
                }
                if let Some(results) = &search_results {
                    for (i, _) in results.iter().take(8) {
                        plot = plot.scatter(vec![positions[*i]], Some(theme.colors.accent));
                        pick_sources.push(vec![*i]);
                    }
                }
                // Small three-axis crosses identify the selected source without relying on hue.
                if let Some(index) = self.timbre_map.selected {
                    for axis in 0..3 {
                        let mut a = positions[index];
                        let mut b = a;
                        a[axis] -= extent * 0.035;
                        b[axis] += extent * 0.035;
                        plot = plot.curve(vec![a, b], Some(theme.colors.text));
                        pick_sources.push(vec![index, index]);
                    }
                    for (i, _) in &neighbors {
                        plot = plot.scatter(vec![positions[*i]], Some(theme.colors.accent));
                        pick_sources.push(vec![*i]);
                    }
                }
                let map_entity = cx.entity().downgrade();
                let pick_sources = std::sync::Arc::new(pick_sources);
                let pick_sounds = map.sounds.len();
                plot = plot
                    .on_pick(move |hit: &Option<PlotPoint>, _, app| {
                        let Some(entity) = map_entity.upgrade() else {
                            return;
                        };
                        let index =
                            pick_source_index(&pick_sources, *hit).filter(|i| *i < pick_sounds);
                        entity.update(app, |this, cx| {
                            this.timbre_map.hovered = index;
                            if let Some(index) = index {
                                this.select_timbre(index, cx);
                            }
                            cx.notify();
                        });
                    })
                    .on_orbit({
                        let map_entity = cx.entity().downgrade();
                        move |angles, _, app| {
                            let Some(entity) = map_entity.upgrade() else {
                                return;
                            };
                            entity.update(app, |this, cx| {
                                this.timbre_map.camera.azimuth = angles.0;
                                this.timbre_map.camera.elevation = angles.1;
                                this.timbre_map.camera.preset = None;
                                cx.notify();
                            });
                        }
                    });
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
                    .child(plot);
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
                let arrangements = SegmentedControl::new(
                    "timbre-axes",
                    [
                        Segment::label(self.t(Key::TimbreAxesSimilarity)),
                        Segment::label(self.t(Key::TimbreAxesCharacter)),
                        Segment::label(self.t(Key::TimbreAxesEnvelope)),
                    ],
                    arrangement,
                )
                .size(ControlSize::Small)
                .on_select(cx.listener(|this, i: &usize, _, cx| {
                    this.timbre_map.arrangement = (*i).min(2);
                    cx.notify();
                }));
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
                // Dragging orbits the map; these are the same turns for a keyboard, and they
                // stay because a map nobody can turn without a pointer is a map half shown.
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
                    .max_h(gpui::rems(6.))
                    .overflow_y_scroll();
                for summary in map.cluster_summaries() {
                    let cluster = summary.id;
                    let name = self.timbre_cluster_name(cluster);
                    let by_ai = self.timbre_cluster_named_by_ai(cluster);
                    let tip = if by_ai {
                        format!(
                            "{name} · {}\n{}",
                            self.t(Key::TimbreMapNamedByAi),
                            summary.examples.join(" · ")
                        )
                    } else {
                        format!("{name}\n{}", summary.examples.join(" · "))
                    };
                    legend = legend.child(
                        div()
                            .id(SharedString::from(format!("timbre-cluster-{cluster}")))
                            .flex()
                            .items_center()
                            .gap(spacing.xs)
                            .max_w(gpui::rems(15.))
                            .tooltip(super::tooltip::keyed_tip(tip, "", &self.theme))
                            .child(
                                atelier::Icon::new(atelier::IconSource::svg(
                                    super::icons::sound_icon_path(
                                        self.timbre_cluster_icon(cluster),
                                    ),
                                ))
                                .size(window.rem_size())
                                .color(self.theme.atelier.categorical(cluster)),
                            )
                            .child(div().min_w_0().truncate().child(cluster_label(name, by_ai)))
                            .child(
                                div().flex_none().child(
                                    Label::new(format!("· {}", summary.sound_count))
                                        .size(TextSize::XSmall)
                                        .color(TextColor::Muted),
                                ),
                            ),
                    );
                }
                // Naming acts on the legend, so it lives in the legend's heading rather than
                // in the window's toolbar, two regions away from what it changes.
                let naming = self.timbre_map.namer.is_some();
                let mut naming_controls = div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap(spacing.sm)
                    .child(
                        Label::new(self.t(if self.timbre_map.names.is_empty() {
                            Key::TimbreMapRepresentativeNames
                        } else {
                            Key::TimbreMapAiNames
                        }))
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                    )
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
                let figure = Card::new()
                    .flex_1()
                    .min_w_0()
                    .min_h(gpui::rems(24.))
                    .p(spacing.lg)
                    .gap(spacing.sm)
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .gap(spacing.sm)
                            .child(arrangements)
                            .child(views)
                            .child(turns),
                    )
                    .child(well)
                    .child(
                        Label::new(self.t(match arrangement {
                            1 => Key::TimbreCharacterGeometry,
                            2 => Key::TimbreEnvelopeGeometry,
                            _ => Key::TimbreMapGeometry,
                        }))
                        .size(TextSize::XSmall)
                        .color(TextColor::Muted),
                    )
                    .child(legend)
                    .child(naming_controls)
                    .children(self.timbre_map.naming_error.as_ref().map(|error| {
                        Label::new(format!("{}: {error}", self.t(Key::TimbreMapNamingFailed)))
                            .size(TextSize::Small)
                            .color(TextColor::Danger)
                    }));

                // The list: every sound, the chosen sound's neighbours, or what the search
                // ranks — and the conditions that made it, each removable.
                let entries: Vec<(usize, Option<f64>)> = match &search_results {
                    Some(results) => results
                        .iter()
                        .take(50)
                        .map(|(i, distance)| (*i, Some(*distance)))
                        .collect(),
                    None => match self.timbre_map.similar_to {
                        Some(anchor) => map
                            .nearest(anchor, 12)
                            .into_iter()
                            .map(|(i, distance)| (i, Some(distance)))
                            .collect(),
                        None => (0..map.sounds.len()).map(|i| (i, None)).collect(),
                    },
                };
                let list_id = if self.timbre_map.similar_to.is_some() {
                    "timbre-neighbors"
                } else {
                    "timbre-all"
                };
                let ranked_by_envelope = !self.timbre_map.envelope.is_empty();
                let finder = div()
                    .id("timbre-finder")
                    .debug_selector(|| "timbre-finder".into())
                    .flex()
                    .flex_col()
                    .gap(spacing.sm)
                    .min_w_0()
                    .min_h_0()
                    .child(SectionHeader::new(self.t(Key::TimbreMapSounds)))
                    .child(search_field)
                    .child(
                        Label::new(search_message)
                            .size(TextSize::XSmall)
                            .color(TextColor::Muted),
                    )
                    .child(self.render_envelope_filter(cx))
                    .child(self.render_timbre_conditions(map, cx))
                    .when(
                        search_results.as_ref().is_some_and(Vec::is_empty),
                        |column| {
                            column.child(
                                Label::new(self.t(Key::TimbreSearchEmpty)).size(TextSize::Small),
                            )
                        },
                    )
                    .child(
                        div()
                            .id(list_id)
                            .debug_selector(move || list_id.into())
                            .flex_1()
                            .min_h(gpui::rems(10.))
                            .p(spacing.xs)
                            .overflow_y_scroll()
                            .children(entries.into_iter().map(|(i, distance)| {
                                let sound = &map.sounds[i];
                                let description = match map.envelopes()[i].as_ref() {
                                    // Ranked by envelope: the numbers that ranked it.
                                    Some(e) if ranked_by_envelope => envelope::summary(&e.summary),
                                    _ if search_results.is_some() => {
                                        let c = map.characters()[i];
                                        format!(
                                            "{} {:.0}% · {} {:.0}% · {} {:.0}%",
                                            self.t(Key::TimbreAxisBrightness),
                                            c.brightness * 100.,
                                            self.t(Key::TimbreAxisAttack),
                                            c.attack * 100.,
                                            self.t(Key::TimbreAxisSustain),
                                            c.sustain * 100.
                                        )
                                    }
                                    _ => self.timbre_cluster_name(map.clusters()[i]),
                                };
                                let mut row = ListItem::new(
                                    SharedString::from(format!("{list_id}-{i}")),
                                    sound.name.clone(),
                                )
                                .selected(self.timbre_map.selected == Some(i))
                                .icon(atelier::IconSource::svg(super::icons::sound_icon_path(
                                    self.library_sound_icon(
                                        &sound.instrument_id,
                                        sound.preset,
                                        &sound.name,
                                    ),
                                )))
                                .icon_color(self.theme.atelier.categorical(map.clusters()[i]))
                                .description(description)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.select_timbre(i, cx);
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

                // The inspector: one sound — what it is, how its envelope runs, how it plays,
                // and the three things to do with it.
                let selected = self.timbre_map.selected.and_then(|i| map.sounds.get(i));
                let target = self
                    .selected_track
                    .filter(|id| self.session.audition_track(Some(*id)) == Some(*id));
                let mut inspector = div()
                    .id("timbre-inspector")
                    .debug_selector(|| "timbre-inspector".into())
                    .flex()
                    .flex_col()
                    .gap(spacing.md)
                    .min_w_0()
                    .min_h_0()
                    .overflow_y_scroll()
                    .child(SectionHeader::new(selected.map_or_else(
                        || self.t(Key::TimbreMapSelect).to_owned(),
                        |s| s.name.clone(),
                    )));
                let shown = self.timbre_map.selected.filter(|i| *i < map.sounds.len());
                if let Some(index) = shown {
                    let c = map.characters()[index];
                    inspector = inspector
                        .child(cluster_label(
                            self.timbre_cluster_name(map.clusters()[index]),
                            self.timbre_cluster_named_by_ai(map.clusters()[index]),
                        ))
                        .child(
                            Label::new(format!(
                                "{} {:.0}% · {} {:.0}%",
                                self.t(Key::TimbreAxisBrightness),
                                c.brightness * 100.,
                                self.t(Key::TimbreAxisNoise),
                                c.noise * 100.
                            ))
                            .size(TextSize::XSmall)
                            .color(TextColor::Muted),
                        );
                }
                // Hearing it and acting on it first, so the commands sit where the eye lands
                // after choosing; the measured envelope follows, with room to draw.
                inspector = inspector
                    .child(self.render_timbre_keyboard(cx))
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
                        Button::new("timbre-find-similar", self.t(Key::TimbreFindSimilar))
                            .disabled(selected.is_none())
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.timbre_map.similar_to = this.timbre_map.selected;
                                cx.notify();
                            })),
                    )
                    .child(
                        Button::new("timbre-attach-chat", self.t(Key::AgentSoundAttach))
                            .disabled(selected.is_none() || !self.can_attach_agent_music())
                            .full_width()
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.attach_timbre_to_chat();
                                cx.notify();
                            })),
                    )
                    .children(selected.map(|sound| self.render_sound_icon_picker(sound, cx)))
                    .children(shown.map(|index| self.render_timbre_envelope(map, index)));

                let width = window.viewport_size().width;
                let body = div()
                    .id("timbre-map-body")
                    .flex()
                    .flex_1()
                    .min_h_0()
                    .gap(spacing.lg)
                    .overflow_y_scroll();
                let body = if width >= window.rem_size() * 64. {
                    body.child(figure)
                        .child(finder.w(gpui::rems(18.)).flex_shrink_0())
                        .child(inspector.w(gpui::rems(18.)).flex_shrink_0())
                } else if width >= window.rem_size() * 48. {
                    // The list goes under the map, so the sound chosen from either keeps a
                    // column of its own and its commands stay in view.
                    body.child(
                        div()
                            .id("timbre-main")
                            .flex()
                            .flex_col()
                            .flex_1()
                            .min_w_0()
                            .gap(spacing.lg)
                            .min_h_0()
                            .overflow_y_scroll()
                            .child(figure)
                            .child(finder.h(gpui::rems(24.)).flex_shrink_0()),
                    )
                    .child(inspector.w(gpui::rems(18.)).flex_shrink_0())
                } else {
                    body.flex_col()
                        .child(figure)
                        .child(finder.w_full().h(gpui::rems(24.)).flex_shrink_0())
                        .child(inspector.w_full().flex_shrink_0())
                };
                content = content.child(body);
            }
        } else if !busy {
            content = content.child(EmptyState::new(
                IconName::Music,
                self.t(Key::TimbreMapReady),
            ));
        }
        // Everything the window reports, on the one line made for it: the scan's progress while
        // it runs, then what the map keeps and leaves out.
        let summary = match (&self.timbre_map.control, &self.timbre_map.map) {
            (Some(control), _) => format!(
                "{} {}/{}",
                self.t(Key::AnalysisRunning),
                control.completed(),
                self.timbre_map.total
            ),
            (None, Some(map)) => format!(
                "{}: {:.0}% · {}: {}",
                self.t(Key::TimbreMapVariance),
                map.explained_variance() * 100.,
                self.t(Key::TimbreMapSkipped),
                map.skipped.len()
            ),
            (None, None) => String::new(),
        };
        let progress = self.timbre_map.control.as_ref().map(|control| {
            div().w(gpui::rems(8.)).child(ProgressBar::new(
                control.completed() as f32 / self.timbre_map.total.max(1) as f32,
            ))
        });
        Some(
            div()
                .id("timbre-map")
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
                        .child(if busy {
                            Button::new("timbre-cancel", self.t(Key::Cancel))
                                .size(ControlSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.timbre_map.cancel();
                                    cx.notify();
                                }))
                        } else {
                            Button::new("timbre-rescan", self.t(Key::TimbreMapRescan))
                                .size(ControlSize::Small)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.scan_timbre_map(cx);
                                    cx.notify();
                                }))
                        })
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
                .child(
                    StatusBar::new()
                        .message(summary)
                        .busy(busy)
                        .trailing(div().children(progress)),
                )
                .into_any_element(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint};
    use gpui::{Entity, TestAppContext, VisualTestContext};

    #[gpui::test]
    fn impression_search_accepts_japanese_without_editing_the_song(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            this.timbre_map.map = Some(Arc::new(
                this.session
                    .timbre_map_job()
                    .unwrap()
                    .run(&TimbreMapControl::default())
                    .unwrap(),
            ));
            this.timbre_map.open = true;
            this.project().clone()
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        click("timbre-search-focus", &mut utility);
        utility.simulate_input("ふんわり");
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.search.content(), "ふんわり");
            assert!(this.taking_text_input());
            assert_eq!(this.project(), &before);
        });
        utility.simulate_keystrokes("enter");
        app.update(&mut utility, |this, cx| {
            assert!(!this.timbre_map.search_focused);
            let map = this.timbre_map.map.as_ref().unwrap();
            let (targets, _) = TimbreTargets::interpret(
                this.timbre_map.search.content(),
                &TimbreTargets::default(),
            )
            .unwrap();
            let results = map.search_character(&targets, &Default::default());
            assert_eq!(results.len(), map.sounds.len());
            assert!(results.windows(2).all(|v| v[0].1 <= v[1].1));
            let positions = map.character_positions();
            this.timbre_map.arrangement = 1;
            assert_eq!(positions.len(), map.sounds.len());
            assert!(positions.iter().flatten().all(|v| (-1. ..=1.).contains(v)));
            cx.notify();
        });
        paint(&app, &mut utility);
        assert!(utility.debug_bounds("timbre-plot").is_some());
        click("timbre-search-clear", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(this.timbre_map.search.content().is_empty());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn the_finder_narrows_by_envelope_and_similarity_and_each_condition_comes_off(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        // The library's name search hands its words to the finder.
        paint(&app, cx);
        click("library-search", cx);
        cx.simulate_input("strings");
        paint(&app, cx);
        app.update(cx, |this, _| {
            // Already measured, so opening the finder does not start a scan of its own.
            this.timbre_map.map = Some(Arc::new(
                this.session
                    .timbre_map_job()
                    .unwrap()
                    .run(&TimbreMapControl::default())
                    .unwrap(),
            ));
        });
        click("library-find-by-sound", cx);
        let before = app.read_with(cx, |this, _| {
            assert!(this.timbre_map.open);
            assert_eq!(this.timbre_map.search.content(), "strings");
            assert!(this.library_search.content().is_empty());
            this.project().clone()
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        // A name word with no impression in it filters by name.
        app.read_with(&utility, |this, _| {
            let map = this.timbre_map.map.as_ref().unwrap();
            let (results, _) = this.timbre_results(map);
            let results = results.unwrap();
            assert!(!results.is_empty());
            assert!(
                results
                    .iter()
                    .all(|(i, _)| map.sounds[*i].name.to_lowercase().contains("strings"))
            );
        });
        click("timbre-search-clear", &mut utility);
        // Slow attacks first, through the disclosure and its segmented choice.
        click("timbre-envelope-filter", &mut utility);
        paint(&app, &mut utility);
        click("timbre-envelope-Attack-3", &mut utility);
        paint(&app, &mut utility);
        app.read_with(&utility, |this, _| {
            let map = this.timbre_map.map.as_ref().unwrap();
            assert_eq!(
                this.timbre_map.envelope.targets().attack_ms.unwrap().min,
                Some(150.)
            );
            let (results, _) = this.timbre_results(map);
            let attacks: Vec<f64> = results
                .unwrap()
                .iter()
                .map(|(i, _)| map.envelopes()[*i].as_ref().unwrap().summary.attack_ms)
                .collect();
            let slowest = attacks.iter().copied().fold(0., f64::max);
            assert_eq!(attacks[0], slowest, "{attacks:?}");
        });
        assert!(
            utility
                .debug_bounds("timbre-envelope-chip-Attack")
                .is_some()
        );
        click("timbre-envelope-chip-Attack", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(this.timbre_map.envelope.is_empty())
        });
        // Choosing a sound draws its measured envelope; "similar" narrows the list to its
        // neighbours until the chip is taken off.
        paint(&app, &mut utility);
        click("timbre-all-0", &mut utility);
        paint(&app, &mut utility);
        assert!(utility.debug_bounds("timbre-envelope").is_some());
        click("timbre-find-similar", &mut utility);
        paint(&app, &mut utility);
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.similar_to, Some(0))
        });
        assert!(utility.debug_bounds("timbre-neighbors").is_some());
        click("timbre-similar-chip", &mut utility);
        paint(&app, &mut utility);
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.similar_to, None);
            assert_eq!(this.project(), &before);
        });
        // The envelope arrangement places every sound inside the map's cube.
        click("timbre-axes-2", &mut utility);
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.arrangement, 2)
        });
        paint(&app, &mut utility);
        assert!(utility.debug_bounds("timbre-plot").is_some());
    }

    #[gpui::test]
    fn manual_categories_survive_ai_and_rescans_and_can_be_reset(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (before, sound, automatic) = app.update(cx, |this, _| {
            let map = this
                .session
                .timbre_map_job()
                .unwrap()
                .run(&TimbreMapControl::default())
                .unwrap();
            let sound = map.sounds[0].clone();
            let key = this
                .session
                .library_sound_key(&sound.instrument_id, sound.preset)
                .unwrap();
            this.settings.sound_icons.remove(&key);
            this.timbre_map.map = Some(Arc::new(map));
            this.timbre_map.selected = Some(0);
            this.timbre_map.open = true;
            let automatic =
                this.library_sound_icon(&sound.instrument_id, sound.preset, &sound.name);
            (this.project().clone(), sound, automatic)
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        click("timbre-edit-icon", &mut utility);
        utility.simulate_keystrokes("tab tab tab tab space");
        utility.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("space").unwrap(),
        });
        app.update(&mut utility, |this, _| {
            let key = this
                .session
                .library_sound_key(&sound.instrument_id, sound.preset)
                .unwrap();
            assert_eq!(
                auris_session::Settings::load().sound_icons.get(&key),
                Some(&SoundIcon::Guitar)
            );
            let map = this.timbre_map.map.as_ref().unwrap();
            let names = map
                .cluster_summaries()
                .iter()
                .map(|summary| ClusterName {
                    id: summary.id,
                    name: format!("AI {}", summary.id),
                    icon: SoundIcon::Voice,
                })
                .collect();
            assert!(
                this.timbre_map
                    .finish_naming(this.timbre_map.naming_generation, Ok(names))
            );
            assert_eq!(
                this.library_sound_icon(&sound.instrument_id, sound.preset, &sound.name),
                SoundIcon::Guitar
            );
            this.timbre_map.clear();
            assert_eq!(
                this.library_sound_icon(&sound.instrument_id, sound.preset, &sound.name),
                SoundIcon::Guitar
            );
            this.timbre_map.map = Some(Arc::new(
                this.session
                    .timbre_map_job()
                    .unwrap()
                    .run(&TimbreMapControl::default())
                    .unwrap(),
            ));
            this.timbre_map.selected = Some(0);
            this.timbre_map.editing_icon = true;
            assert_eq!(this.project(), &before);
        });
        paint(&app, &mut utility);
        let inspector = utility.debug_bounds("timbre-inspector").unwrap();
        utility.simulate_event(gpui::ScrollWheelEvent {
            position: gpui::point(
                inspector.right() - gpui::px(4.),
                inspector.top() + gpui::px(4.),
            ),
            delta: gpui::ScrollDelta::Pixels(gpui::point(gpui::px(0.), gpui::px(-500.))),
            ..Default::default()
        });
        paint(&app, &mut utility);
        assert!(inspector.contains(&utility.debug_bounds("timbre-icon-auto").unwrap().center()));
        click("timbre-icon-auto", &mut utility);
        app.update(&mut utility, |this, _| {
            let key = this
                .session
                .library_sound_key(&sound.instrument_id, sound.preset)
                .unwrap();
            assert!(
                !auris_session::Settings::load()
                    .sound_icons
                    .contains_key(&key)
            );
            assert_eq!(
                this.library_sound_icon(&sound.instrument_id, sound.preset, &sound.name),
                automatic
            );
            let previous = this.settings.clone();
            this.correct_sound_icon_with(&sound, Some(SoundIcon::Brass), |_| Err("disk full"));
            assert_eq!(this.settings, previous);
            assert_eq!(this.timbre_map.error.as_deref(), Some("disk full"));
            assert_eq!(this.project(), &before);
        });
    }

    fn map_window(app: &Entity<AurisApp>, cx: &mut VisualTestContext) -> VisualTestContext {
        let handle = app.read_with(cx, |app, _| {
            app.auxiliary_windows[&crate::auxiliary_window::Surface::TimbreMap]
        });
        VisualTestContext::from_window(handle.into(), cx)
    }

    #[test]
    fn plot_pick_series_map_includes_highlight_series() {
        let series = vec![vec![4, 7], vec![2, 2], vec![9]];
        assert_eq!(
            pick_source_index(
                &series,
                Some(PlotPoint {
                    series: 0,
                    index: 1
                })
            ),
            Some(7)
        );
        assert_eq!(
            pick_source_index(
                &series,
                Some(PlotPoint {
                    series: 1,
                    index: 0
                })
            ),
            Some(2)
        );
        assert_eq!(
            pick_source_index(
                &series,
                Some(PlotPoint {
                    series: 2,
                    index: 0
                })
            ),
            Some(9)
        );
        assert_eq!(
            pick_source_index(
                &series,
                Some(PlotPoint {
                    series: 3,
                    index: 0
                })
            ),
            None
        );
        assert_eq!(pick_source_index(&series, None), None);
    }

    #[gpui::test]
    fn map_previews_and_chromatic_keys_work_without_a_track(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            let tracks: Vec<_> = this.project().tracks.iter().map(|track| track.id).collect();
            for track in tracks {
                this.session.remove_track(track).unwrap();
            }
            this.selected_track = None;
            this.session.forget_history();
            assert!(this.project().tracks.is_empty());
            this.timbre_map.map = Some(Arc::new(
                this.session
                    .timbre_map_job()
                    .unwrap()
                    .run(&TimbreMapControl::default())
                    .unwrap(),
            ));
            this.timbre_map.open = true;
            this.project().clone()
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        click("timbre-all-0", &mut utility);
        utility.run_until_parked();
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.preview.pitch, Some(60));
            assert!(this.timbre_map.preview.status.is_some());
            assert!(this.timbre_map.error.is_none());
        });
        for pitch in 60..=72 {
            paint(&app, &mut utility);
            click(
                Box::leak(format!("timbre-key-{pitch}").into_boxed_str()),
                &mut utility,
            );
            utility.run_until_parked();
            app.read_with(&utility, |this, _| {
                assert_eq!(this.timbre_map.preview.pitch, Some(pitch));
                assert!(this.timbre_map.preview.status.is_some());
                assert!(this.timbre_map.error.is_none());
                assert_eq!(this.project(), &before);
            });
        }
        click("timbre-stop", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(this.timbre_map.preview.status.is_none())
        });
        app.update(&mut utility, |this, cx| {
            this.preview_timbre_note(61, cx);
            this.close_timbre_map();
        });
        utility.run_until_parked();
        app.read_with(&utility, |this, _| {
            assert!(this.timbre_map.preview.status.is_none());
            assert!(this.timbre_map.preview.control.is_none());
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn map_choice_can_be_attached_without_applying_it_to_a_track(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            let map = this
                .session
                .timbre_map_job()
                .unwrap()
                .run(&TimbreMapControl::default())
                .unwrap();
            assert!(!map.sounds.is_empty());
            this.timbre_map.map = Some(Arc::new(map));
            this.timbre_map.selected = Some(0);
            this.timbre_map.open = true;
            this.project().clone()
        });
        paint(&app, cx);
        let mut utility = map_window(&app, cx);
        click("timbre-attach-chat", &mut utility);
        app.read_with(&utility, |this, _| {
            assert!(!this.timbre_map.open);
            assert!(this.panels.is_open(crate::dock::Panel::Agent));
            assert_eq!(this.project(), &before);
        });
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
            assert_eq!(annotation.color, this.theme.atelier.categorical(cluster));
            assert!(!annotation.name.is_empty());
            assert!(!annotation.by_ai);
            assert!(this.library_timbre_cluster("unmeasured", None).is_none());
            (this.project().clone(), index)
        });
        paint(&app, cx);
        // GPUI's debug selector API requires a static string, including for dynamic rows.
        let mut utility = map_window(&app, cx);
        // Pick a known projected point through Plot3d's native release handler, then verify that
        // orbiting the map keeps the selection and does not author document changes.
        let (positions, extent, clusters, azimuth, elevation) =
            app.read_with(&utility, |this, _| {
                let map = this.timbre_map.map.as_ref().unwrap();
                let (positions, extent) = plot_data(map.positions());
                (
                    positions,
                    extent,
                    map.clusters().to_vec(),
                    this.timbre_map.camera.azimuth,
                    this.timbre_map.camera.elevation,
                )
            });
        let mut native_plot = Plot3d::new()
            .x_range(-extent, extent)
            .y_range(-extent, extent)
            .z_range(-extent, extent)
            .view(azimuth, elevation);
        let mut cluster_ids = clusters.clone();
        cluster_ids.sort_unstable();
        cluster_ids.dedup();
        for cluster in cluster_ids {
            let points = clusters
                .iter()
                .enumerate()
                .filter_map(|(i, value)| (*value == cluster).then_some(positions[i]))
                .collect();
            native_plot = native_plot.scatter(points, None);
        }
        let plot_bounds = utility.debug_bounds("timbre-plot").unwrap();
        let point = native_plot.project(plot_bounds, positions[index]).0;
        crate::harness::click_at(&mut utility, point, gpui::Modifiers::none());
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.selected, Some(index));
        });
        let center = utility.debug_bounds("timbre-plot").unwrap().center();
        crate::harness::drag(
            &mut utility,
            center,
            center + gpui::point(gpui::px(50.), gpui::px(20.)),
        );
        app.read_with(&utility, |this, _| {
            assert_eq!(this.timbre_map.selected, Some(index));
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
            if crate::app::AurisApp::wants_menu_bar() {
                utility.simulate_keystrokes("tab");
            }
            // Rescan, Close, three arrangements, four views, left and right. AI naming sits
            // under the map with the legend it names, so it no longer comes first.
            let _ = configured;
            utility.simulate_keystrokes("tab tab tab tab tab tab tab tab tab tab tab");
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
                    icon: SoundIcon::Piano,
                    name: format!("Named cluster {}", cluster.id),
                })
                .collect();
            let sound = map.sounds[index].clone();
            let cluster = map.clusters()[index];
            let generation = this.timbre_map.naming_generation;
            assert!(this.timbre_map.finish_naming(generation, Ok(names)));
            let tag = this
                .library_timbre_cluster(&sound.instrument_id, sound.preset)
                .unwrap();
            assert_eq!(tag.name, format!("Named cluster {cluster}"));
            assert!(tag.by_ai, "an AI name carries the agent's mark");
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
                    icon: SoundIcon::Guitar,
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
