//! Background chord auditions and explicit adoption of the rendered candidate.

use super::{lyrics::section_label, song_spec};
use crate::app::AurisApp;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::widgets::{ButtonStyle, button};
use auris_i18n::Key;
use auris_session::{ChordPreview, ChordPreviewJob, prelude::SongSpec};
use gpui::{Context, IntoElement, div, prelude::*};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

#[derive(Default)]
pub(crate) struct SongPreviewState {
    section: Option<String>,
    source: Option<SongSpec>,
    revision: u64,
    sample_rate: f64,
    generation: u64,
    cancel: Arc<AtomicBool>,
    running: bool,
    ready: Option<ChordPreview>,
    playing_until: Option<Instant>,
    message: Option<String>,
}

impl AurisApp {
    /// Cancels the worker and audio without changing the draft's chosen chords.
    pub(crate) fn clear_chord_preview(&mut self) {
        let state = &mut self.song_preview;
        state.cancel.store(true, Ordering::Relaxed);
        state.generation = state.generation.wrapping_add(1);
        if state.source.is_some() {
            self.session.stop_chord_preview();
        }
        state.source = None;
        state.ready = None;
        state.running = false;
        state.playing_until = None;
        state.message = None;
    }

    /// Invalidates work before its result or sound can outlive an edit or a closed sheet.
    pub(crate) fn reconcile_chord_preview(&mut self) {
        if let Some(source) = &self.song_preview.source {
            let current = self.song_sheet.as_ref().map(song_spec);
            if current.as_ref() != Some(source)
                || self.session.revision() != self.song_preview.revision
                || self.session.sample_rate() != self.song_preview.sample_rate
            {
                self.clear_chord_preview();
            }
        }
        if self
            .song_preview
            .playing_until
            .is_some_and(|until| Instant::now() >= until)
        {
            self.song_preview.playing_until = None;
        }
    }

    pub(crate) fn select_chord_preview_section(&mut self, section: String) {
        self.clear_chord_preview();
        self.song_preview.section = Some(section);
    }

    fn preview_section(&self, spec: &SongSpec) -> Option<String> {
        self.song_preview
            .section
            .as_ref()
            .filter(|name| spec.form.contains(name))
            .or_else(|| spec.form.iter().find(|name| name.as_str() == "verse"))
            .or_else(|| spec.form.first())
            .cloned()
    }

    fn play_ready_chords(&mut self) {
        if let Some(preview) = &self.song_preview.ready {
            match self.session.play_chord_preview(preview) {
                Ok(()) => {
                    self.song_preview.playing_until = Some(
                        Instant::now() + Duration::from_secs_f64(preview.buffer.duration_seconds()),
                    )
                }
                Err(error) => {
                    self.song_preview.message =
                        Some(format!("{}: {error}", self.t(Key::SongPreviewFailed)))
                }
            }
        }
    }

    pub(crate) fn preview_song_chords(&mut self, alternative: bool, cx: &mut Context<Self>) {
        self.reconcile_chord_preview();
        if self.compose_progress.is_some() {
            return;
        }
        if !alternative && (self.song_preview.running || self.song_preview.playing_until.is_some())
        {
            if self.song_preview.running {
                self.clear_chord_preview();
            } else {
                self.session.stop_chord_preview();
                self.song_preview.playing_until = None;
            }
            cx.notify();
            return;
        }
        if !alternative && self.song_preview.ready.is_some() {
            self.play_ready_chords();
            cx.notify();
            return;
        }
        let Some(spec) = self.song_sheet.as_ref().map(song_spec) else {
            return;
        };
        let Some(section) = self.preview_section(&spec) else {
            return;
        };
        let fallback = self.song_preview.ready.take();
        let previous = fallback.as_ref().map(|preview| preview.chord_names.clone());
        self.clear_chord_preview();
        let state = &mut self.song_preview;
        state.source = Some(spec.clone());
        state.section = Some(section.clone());
        state.sample_rate = self.session.sample_rate();
        state.revision = self.session.revision();
        state.cancel = Arc::new(AtomicBool::new(false));
        state.running = true;
        let generation = state.generation;
        let cancel = Arc::clone(&state.cancel);
        let seed = alternative.then(|| spec.seed.wrapping_add(generation));
        let job = ChordPreviewJob::new(spec, section, seed, state.sample_rate)
            .avoiding(if alternative { previous } else { None });
        cx.spawn(async move |this, cx| {
            let rendered = cx
                .background_executor()
                .spawn(async move { job.run(&cancel) })
                .await;
            let _ = this.update(cx, |this, cx| {
                this.reconcile_chord_preview();
                if this.song_preview.generation != generation {
                    return;
                }
                this.song_preview.running = false;
                match rendered {
                    Ok(preview) => {
                        this.song_preview.ready = Some(preview);
                        this.play_ready_chords();
                    }
                    Err(error) => {
                        this.song_preview.ready = fallback;
                        this.song_preview.message =
                            Some(format!("{}: {error}", this.t(Key::SongPreviewFailed)));
                    }
                }
                cx.notify();
            });
        })
        .detach();
        cx.notify();
    }

    pub(crate) fn adopt_chord_preview(&mut self, cx: &mut Context<Self>) {
        self.reconcile_chord_preview();
        let Some(dials) = self.song_sheet.as_mut() else {
            return;
        };
        let Some(preview) = &self.song_preview.ready else {
            return;
        };
        let mut spec = song_spec(dials);
        if !preview.apply_to(&mut spec) {
            return;
        }
        // Only harmony changes. The GUI's automatic mood/tempo choices remain active.
        dials.charts = spec
            .chart_order
            .iter()
            .filter_map(|name| {
                spec.charts
                    .get(name)
                    .map(|chart| (name.clone(), chart.clone()))
            })
            .collect();
        for section in &mut dials.sections {
            if let Some(saved) = spec.sections.get(&section.name) {
                section.chords = saved.chords.clone();
            }
        }
        self.clear_chord_preview();
        self.song_preview.source = self.song_sheet.as_ref().map(song_spec);
        self.song_preview.message = Some(self.t(Key::SongPreviewAdopted).to_string());
        self.set_status(self.t(Key::SongPreviewAdopted));
        cx.notify();
    }

    pub(super) fn render_chord_preview(
        &self,
        spec: &SongSpec,
        cx: &mut Context<Self>,
    ) -> impl IntoElement + use<> {
        let theme = &self.theme;
        let section = self.preview_section(spec);
        let label = section
            .as_deref()
            .map(|s| section_label(self, s))
            .unwrap_or_default();
        let busy = self.song_preview.running || self.song_preview.playing_until.is_some();
        let mut row = div()
            .flex()
            .flex_wrap()
            .items_center()
            .gap_2()
            .child(button(
                "song-preview-section",
                label,
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                Self::opens_menu(cx, |this, anchor| {
                    let mut menu = ContextMenu::new(anchor, this.t(Key::SongPreviewSection));
                    if let Some(spec) = this.song_sheet.as_ref().map(song_spec) {
                        let mut seen = std::collections::HashSet::new();
                        for name in &spec.form {
                            if seen.insert(name) {
                                menu = menu.toggle(
                                    section_label(this, name),
                                    MenuCommand::SongPreviewSection(name.clone()),
                                    this.preview_section(&spec).as_ref() == Some(name),
                                );
                            }
                        }
                    }
                    menu
                }),
            ))
            .child(button(
                "song-preview-play",
                self.t(if busy {
                    Key::SongPreviewStop
                } else {
                    Key::SongPreviewPlay
                }),
                ButtonStyle::Normal,
                busy,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| this.preview_song_chords(false, cx)),
            ))
            .child(button(
                "song-preview-another",
                self.t(Key::SongPreviewAnother),
                ButtonStyle::Normal,
                false,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| this.preview_song_chords(true, cx)),
            ));
        if self.song_preview.ready.is_some() {
            row = row.child(button(
                "song-preview-adopt",
                self.t(Key::SongPreviewAdopt),
                ButtonStyle::Primary,
                false,
                theme.accent,
                theme,
                cx.listener(|this, _, _, cx| this.adopt_chord_preview(cx)),
            ));
        }
        let text = if let Some(message) = &self.song_preview.message {
            message.clone()
        } else if self.song_preview.running {
            self.t(Key::SongPreviewRendering).to_string()
        } else if let Some(preview) = &self.song_preview.ready {
            format!(
                "{} {} · {}",
                preview.bars,
                self.t(Key::SongBarsUnit),
                preview.chord_names.join(" | ")
            )
        } else {
            self.t(Key::SongPreviewHint).to_string()
        };
        div()
            .debug_selector(|| "song-chord-preview".to_string())
            .flex()
            .flex_col()
            .flex_shrink_0()
            .gap_1()
            .child(row)
            .child(div().text_xs().text_color(theme.text_muted).child(text))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{choose, click, open, paint, resize};
    use gpui::{TestAppContext, px, size};

    fn request() -> SongSpec {
        SongSpec::parse("key = 'D dorian'\ntempo = 240\nform = 'verse chorus'\nending = 'none'\n[section.verse]\nbars = 4\n[section.chorus]\nbars = 4").unwrap()
    }

    #[gpui::test]
    fn audition_stop_adopt_and_create_keep_the_heard_chords(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            this.song_sheet = Some(super::super::song_dials(&request()));
            this.project().clone()
        });
        paint(&app, cx);
        click("song-preview-play", cx);
        paint(&app, cx);
        let names = app.read_with(cx, |this, _| {
            assert_eq!(this.project(), &before);
            assert!(!this.session.can_undo());
            assert!(this.song_preview.playing_until.is_some());
            this.song_preview
                .ready
                .as_ref()
                .unwrap()
                .chord_names
                .clone()
        });
        click("song-preview-play", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_preview.playing_until.is_none());
            assert!(
                this.song_preview.ready.is_some(),
                "stopping must retain the candidate"
            );
        });
        click("song-preview-adopt", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.project(), &before);
            let spec = song_spec(this.song_sheet.as_ref().unwrap());
            assert!(!spec.charts[&spec.sections["verse"].chords].is_unwritten());
            assert!(this.song_preview.message.is_some());
        });
        click("song-sheet-write", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            let saved = SongSpec::parse(this.project().song_spec.as_deref().unwrap()).unwrap();
            let chart = &saved.charts[&saved.sections["verse"].chords];
            let heard: Vec<_> = chart
                .resolve(saved.key, saved.meter.ticks_per_bar())
                .iter()
                .map(|e| e.chord.name_in(e.key))
                .collect();
            assert_eq!(heard.join(" / "), names.join(" / "));
            assert!(this.song_preview.ready.is_none());
        });
    }

    #[gpui::test]
    fn edits_section_changes_and_cancel_reject_in_flight_results(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.song_sheet = Some(super::super::song_dials(&request()))
        });
        paint(&app, cx);
        app.update(cx, |this, cx| {
            this.preview_song_chords(false, cx);
            assert!(this.song_preview.running);
            this.song_sheet.as_mut().unwrap().tempo = 180.0;
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| assert!(this.song_preview.ready.is_none()));
        click("song-preview-section", cx);
        paint(&app, cx);
        choose(&app, cx, &MenuCommand::SongPreviewSection("chorus".into()));
        paint(&app, cx);
        click("song-preview-another", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.song_preview.ready.as_ref().unwrap().section, "chorus")
        });
        app.update(cx, |this, cx| {
            this.preview_song_chords(true, cx);
            this.song_sheet = None;
        });
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_preview.ready.is_none());
            assert!(!this.song_preview.running);
        });
    }

    #[gpui::test]
    fn preview_controls_fit_both_languages_and_small_windows(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        app.update(cx, |this, _| {
            this.song_sheet = Some(super::super::song_dials(&request()))
        });
        for language in [
            auris_i18n::Language::English,
            auris_i18n::Language::Japanese,
        ] {
            app.update(cx, |this, _| this.language = language);
            for width in [640.0, 900.0, 1280.0] {
                resize(&app, cx, size(px(width), px(650.0)));
                for selector in [
                    "song-preview-section",
                    "song-preview-play",
                    "song-preview-another",
                    "song-sheet-write",
                ] {
                    let panel = cx.debug_bounds("song-sheet-panel").unwrap();
                    let bounds = cx.debug_bounds(selector).unwrap();
                    assert!(bounds.left() >= panel.left() && bounds.right() <= panel.right());
                    assert!(bounds.top() >= panel.top() && bounds.bottom() <= panel.bottom());
                }
            }
        }
    }

    #[gpui::test]
    fn a_failed_alternative_keeps_the_last_audition_available(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let mut spec = request();
        spec.sections.get_mut("verse").unwrap().bars = 1;
        spec.mood.tension = 0.0;
        app.update(cx, |this, _| {
            this.song_sheet = Some(super::super::song_dials(&spec))
        });
        paint(&app, cx);
        click("song-preview-play", cx);
        paint(&app, cx);
        let names = app.read_with(cx, |this, _| {
            this.song_preview
                .ready
                .as_ref()
                .unwrap()
                .chord_names
                .clone()
        });
        click("song-preview-another", cx);
        paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.song_preview.message.is_some());
            assert_eq!(this.song_preview.ready.as_ref().unwrap().chord_names, names);
        });
    }
}
