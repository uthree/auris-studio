//! Worker-based CPU recognition and explicit acceptance in the inspector.

use crate::{
    app::AurisApp,
    ui::{
        context_menu::MenuCommand,
        prompt::Prompt,
        widgets::{ButtonStyle, button},
    },
};
use auris_i18n::Key;
use auris_session::prelude::*;
use auris_session::{
    AnalysisControl, AudioOptions, ChordAnalysisReport, ChordState, ClipAudioAnalysis, SessionError,
};
use gpui::{AnyElement, Context, IntoElement, SharedString, div, prelude::*};

/// A report awaiting the user's choice.
#[derive(Clone, Debug)]
pub(crate) enum MusicReport {
    /// Written-note harmony.
    Chords(ChordAnalysisReport),
    /// Trimmed audio-source measurements and optional notes.
    Audio(Box<ClipAudioAnalysis>),
    /// Local model's instrument-presence hypotheses, without an acceptance edit.
    Instruments(Box<auris_session::ClipInstrumentAnalysis>),
    /// Optional noncommercial multi-instrument transcription.
    Mixture(Box<auris_session::ClipMixtureAnalysis>),
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{click, open, paint};
    use gpui::TestAppContext;

    #[gpui::test]
    fn mixture_notice_is_required_each_time_and_cancel_starts_no_worker(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        for _ in 0..2 {
            app.update(cx, |this, cx| {
                this.run_menu_command(MenuCommand::TranscribeMixture(ClipId(999)), cx);
                assert!(this.music_analysis.control.is_none());
                assert!(this.music_analysis.report.is_none());
                assert!(matches!(
                    this.prompt.as_ref().map(|p| &p.body),
                    Some(crate::ui::prompt::PromptBody::Ask(
                        crate::ui::prompt::Question::Muscriptor { .. }
                    ))
                ));
            });
            paint(&app, cx);
            click("prompt-cancel", cx);
            app.read_with(cx, |this, _| {
                assert!(this.prompt.is_none());
                assert!(this.music_analysis.control.is_none());
            });
        }
    }

    fn written_triad(app: &mut AurisApp) -> ClipId {
        app.panels = crate::dock::PanelLayout::default();
        let track = app
            .session
            .add_default_instrument_track("Analysis fixture")
            .unwrap();
        let clip = app
            .session
            .add_midi_clip(track, "C major", Ticks::ZERO, Ticks::QUARTER)
            .unwrap();
        for pitch in [60, 64, 67] {
            app.session
                .add_note(clip, Note::new(pitch, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        }
        app.selected_track = None;
        clip
    }

    #[gpui::test]
    fn inspector_button_reports_without_editing_then_accepts_with_undo(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let before = app.update(cx, |this, _| {
            written_triad(this);
            this.project().clone()
        });
        paint(&app, cx);
        click("music-all", cx);
        cx.run_until_parked();
        app.read_with(cx, |this, _| {
            assert!(matches!(
                this.music_analysis.report,
                Some(MusicReport::Chords(_))
            ));
            assert_eq!(this.project(), &before);
        });
        paint(&app, cx);
        click("music-apply", cx);
        app.update(cx, |this, _| {
            assert!(this.music_analysis.report.is_none());
            assert_eq!(
                this.session
                    .harmony()
                    .chord_at(Ticks::ZERO)
                    .unwrap()
                    .to_string(),
                "C"
            );
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
    }

    #[gpui::test]
    fn stale_cancelled_and_replaced_documents_do_not_receive_worker_results(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = open(cx);
        app.update(cx, |this, cx| {
            let clip = written_triad(this);
            this.begin_chord_analysis(None, cx);
            this.session
                .add_note(clip, Note::new(61, Ticks::ZERO, Ticks::QUARTER))
                .unwrap();
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert!(this.music_analysis.report.is_none());
            this.begin_chord_analysis(None, cx);
            this.run_menu_command(MenuCommand::CancelMusicAnalysis, cx);
        });
        cx.run_until_parked();
        app.update(cx, |this, cx| {
            assert!(this.music_analysis.report.is_none());
            this.begin_chord_analysis(None, cx);
            this.new_project();
        });
        cx.run_until_parked();
        app.read_with(cx, |this, _| assert!(this.music_analysis.report.is_none()));
    }

    #[gpui::test]
    fn audio_worker_creates_an_editable_track_without_changing_its_source(cx: &mut TestAppContext) {
        let (app, cx) = open(cx);
        let (before, previous_clip) = app.update(cx, |this, cx| {
            let previous_clip = written_triad(this);
            let rate = this.project().sample_rate;
            let samples = (0..rate as usize)
                .map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / rate).sin() as f32 * 0.4)
                .collect();
            let buffer = AudioBuffer::from_planar(vec![samples], rate).unwrap();
            let clip = this
                .session
                .place_audio(std::path::Path::new("mono.wav"), buffer, Ticks::ZERO)
                .unwrap();
            this.select_track(this.session.track_of_clip(clip).unwrap());
            this.select_clip(Some(clip));
            this.run_menu_command(
                MenuCommand::AnalyzeAudio {
                    clip,
                    transcribe: true,
                },
                cx,
            );
            (this.project().clone(), previous_clip)
        });
        cx.run_until_parked();
        // Selecting another clip does not invalidate the worker's source.
        // Accepting the draft must then move every editor to the new track together.
        app.update(cx, |this, _| {
            this.select_track(this.session.track_of_clip(previous_clip).unwrap());
            this.selected_notes.insert(0);
            assert_eq!(this.selected_clip, Some(previous_clip));
        });
        paint(&app, cx);
        click("music-notes", cx);
        app.update(cx, |this, _| {
            assert_eq!(this.project().tracks.len(), before.tracks.len() + 1);
            let selected_track = this.selected_track.unwrap();
            assert!(before.track(selected_track).is_none());
            let selected_clip = this
                .selected_clip
                .expect("the new draft is open for editing");
            assert_eq!(
                this.session.track_of_clip(selected_clip),
                Some(selected_track)
            );
            assert_ne!(selected_clip, previous_clip);
            assert_eq!(
                this.selected_clips.iter().copied().collect::<Vec<_>>(),
                [selected_clip]
            );
            assert!(this.selected_notes.is_empty());
            this.session.undo();
            assert_eq!(this.project(), &before);
        });
    }
}

/// One active worker and its last result, owned by the frontend.
#[derive(Default)]
pub(crate) struct MusicAnalysisState {
    generation: u64,
    control: Option<AnalysisControl>,
    /// Current draft, cleared when the document is replaced or the draft is accepted.
    pub(crate) report: Option<MusicReport>,
}

impl MusicAnalysisState {
    /// Cancels pending work and invalidates any completion already queued for the UI.
    pub(crate) fn cancel(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        if let Some(control) = self.control.take() {
            control.cancel();
        }
    }
}

impl AurisApp {
    /// Shows the model-use notice before loading a model or source audio.
    pub(crate) fn request_mixture_transcription(&mut self, clip: ClipId) {
        self.music_analysis.cancel();
        self.music_analysis.report = None;
        self.open_prompt(Prompt::ask(
            self.t(Key::MenuTranscribeMixture),
            crate::ui::prompt::Question::Muscriptor {
                clip,
                generation: self.music_analysis.generation,
            },
        ));
    }

    /// Continues only the invocation acknowledged in the license sheet.
    pub(crate) fn choose_mixture_model(
        &mut self,
        clip: ClipId,
        generation: u64,
        cx: &mut Context<Self>,
    ) {
        if generation != self.music_analysis.generation {
            return;
        }
        let job = match self
            .session
            .audio_analysis_job(clip, AudioOptions::default())
        {
            Ok(job) => job,
            Err(e) => {
                self.set_failed_status(e.to_string());
                return;
            }
        };
        let title = self.t(Key::MuscriptorModel).to_string();
        cx.spawn(async move |this, cx| {
            let Some(file) = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("MuScriptor Small ONNX", &["onnx"])
                .pick_file()
                .await
            else {
                return;
            };
            let options = auris_session::MixtureOptions {
                model: file.path().to_path_buf(),
                acknowledge_noncommercial: true,
            };
            let _ = this.update(cx, |this, cx| {
                if generation != this.music_analysis.generation {
                    return;
                }
                this.start_music_worker(
                    move |control| {
                        job.run_mixture(&options, control)
                            .map(|r| MusicReport::Mixture(Box::new(r)))
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    /// Schedules analysis of all pitched tracks or the selected note track.
    pub(crate) fn begin_chord_analysis(&mut self, track: Option<TrackId>, cx: &mut Context<Self>) {
        let tracks: Vec<_> = track.into_iter().collect();
        let (from, to) = match track.and_then(|id| self.project().track(id)) {
            Some(track) => {
                let clips: Vec<_> = track.kind.note_clips().into_iter().flatten().collect();
                (
                    clips.iter().map(|c| c.start).min().unwrap_or(Ticks::ZERO),
                    clips
                        .iter()
                        .map(|c| c.sounding_end())
                        .max()
                        .unwrap_or(Ticks::ZERO),
                )
            }
            None => (Ticks::ZERO, self.project().end_tick()),
        };
        match self
            .session
            .chord_analysis_job(&tracks, from, to, Default::default())
        {
            Ok(job) => self
                .start_music_worker(move |control| job.run(control).map(MusicReport::Chords), cx),
            Err(e) => self.set_failed_status(e.to_string()),
        }
    }

    /// Schedules analysis of an audio clip's trimmed source.
    pub(crate) fn begin_audio_analysis(
        &mut self,
        clip: ClipId,
        transcribe: bool,
        cx: &mut Context<Self>,
    ) {
        match self
            .session
            .audio_analysis_job(clip, AudioOptions { transcribe })
        {
            Ok(job) => self.start_music_worker(
                move |control| job.run(control).map(|r| MusicReport::Audio(Box::new(r))),
                cx,
            ),
            Err(e) => self.set_failed_status(e.to_string()),
        }
    }

    /// Prompts for a prepared model; the source snapshot is captured before the dialog.
    pub(crate) fn choose_instrument_model(&mut self, clip: ClipId, cx: &mut Context<Self>) {
        let job = match self
            .session
            .audio_analysis_job(clip, AudioOptions::default())
        {
            Ok(job) => job,
            Err(e) => {
                self.set_failed_status(e.to_string());
                return;
            }
        };
        self.music_analysis.cancel();
        self.music_analysis.report = None;
        let generation = self.music_analysis.generation;
        let title = self.t(Key::DialogYamnetModel).to_string();
        cx.spawn(async move |this, cx| {
            let Some(handle) = rfd::AsyncFileDialog::new()
                .set_title(title)
                .add_filter("ONNX", &["onnx"])
                .pick_file()
                .await
            else {
                return;
            };
            let path = handle.path().to_path_buf();
            let _ = this.update(cx, |this, cx| {
                if generation != this.music_analysis.generation {
                    return;
                }
                this.start_music_worker(
                    move |control| {
                        job.run_instruments(&path, 0.2, control)
                            .map(|r| MusicReport::Instruments(Box::new(r)))
                    },
                    cx,
                );
            });
        })
        .detach();
    }

    fn start_music_worker(
        &mut self,
        work: impl FnOnce(&AnalysisControl) -> Result<MusicReport, SessionError> + Send + 'static,
        cx: &mut Context<Self>,
    ) {
        self.music_analysis.cancel();
        self.music_analysis.report = None;
        let generation = self.music_analysis.generation;
        let control = AnalysisControl::default();
        self.music_analysis.control = Some(control.clone());
        self.set_status(self.t(Key::AnalysisRunning));
        cx.spawn(async move |this, cx| {
            let result = cx
                .background_executor()
                .spawn(async move { work(&control) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if this.music_analysis.generation != generation {
                    return;
                }
                this.music_analysis.control = None;
                match result {
                    Ok(report) if this.music_report_current(&report) => {
                        this.music_analysis.report = Some(report);
                        this.set_status(this.t(Key::AnalysisReady));
                    }
                    Ok(_) => this.set_failed_status(this.t(Key::AnalysisStale)),
                    Err(e) => this.set_failed_status(e.to_string()),
                }
                cx.notify();
            });
        })
        .detach();
        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(150))
                    .await;
                let keep = this
                    .update(cx, |this, cx| {
                        let active = this.music_analysis.generation == generation
                            && this.music_analysis.control.is_some();
                        if active {
                            cx.notify();
                        }
                        active
                    })
                    .unwrap_or(false);
                if !keep {
                    break;
                }
            }
        })
        .detach();
    }

    fn music_report_current(&self, report: &MusicReport) -> bool {
        match report {
            MusicReport::Chords(r) => self.session.chord_analysis_is_current(r),
            MusicReport::Audio(r) => self.session.audio_analysis_is_current(r),
            MusicReport::Instruments(r) => self.session.instrument_analysis_is_current(r),
            MusicReport::Mixture(r) => self.session.mixture_analysis_is_current(r),
        }
    }

    /// Applies the user's chosen report action as an undoable session command.
    pub(crate) fn accept_music_analysis(
        &mut self,
        choice: Option<(usize, usize)>,
        notes: bool,
        tempo: Option<usize>,
    ) {
        let Some(report) = self.music_analysis.report.clone() else {
            return;
        };
        let name = self.t(Key::AnalysisDraft).to_string();
        let outcome = match &report {
            MusicReport::Instruments(_) => return,
            MusicReport::Mixture(r) if notes => {
                self.session.create_clip_mixture_tracks(r).map(|tracks| {
                    if let Some(track) = tracks.first().copied() {
                        self.select_track(track);
                    }
                })
            }
            MusicReport::Mixture(_) => return,
            MusicReport::Chords(r) => match choice {
                Some((segment, candidate)) => {
                    self.session.apply_chord_candidate(r, segment, candidate)
                }
                None => self.session.apply_chord_analysis(r),
            }
            .map(|_| ()),
            MusicReport::Audio(r) if notes => self
                .session
                .create_clip_transcription_track(r, &name)
                .map(|(track, _)| {
                    self.select_track(track);
                }),
            MusicReport::Audio(r) if tempo.is_some() => {
                self.session.apply_audio_source_tempo(r, tempo.unwrap())
            }
            MusicReport::Audio(r) => self
                .session
                .audio_chord_report(r)
                .and_then(|r| self.session.apply_chord_analysis(&r))
                .map(|_| ()),
        };
        match outcome {
            Ok(()) => {
                self.music_analysis.report = None;
                self.set_status(self.t(if notes {
                    Key::AnalysisCreateNotes
                } else if tempo.is_some() {
                    Key::AnalysisSetSourceTempo
                } else {
                    Key::AnalysisApplyChords
                }));
            }
            Err(e) => self.set_failed_status(e.to_string()),
        }
    }

    /// Opens the report's bounded textual preview.
    pub(crate) fn view_music_analysis(&mut self) {
        let Some(report) = &self.music_analysis.report else {
            return;
        };
        let mut lines: Vec<SharedString> = vec![self.t(Key::AnalysisScores).into()];
        let reading = |r: &auris_session::ChordReading| {
            if r.state == ChordState::NoChord {
                return self.t(Key::AnalysisNoChord).to_string();
            }
            let alternatives = r
                .candidates
                .iter()
                .map(|c| format!("{} ({:.2})", c.symbol, c.score))
                .collect::<Vec<_>>()
                .join(" / ");
            format!(
                "{}{}",
                if r.state == ChordState::Unknown {
                    format!("{}: ", self.t(Key::AnalysisUnknown))
                } else {
                    String::new()
                },
                alternatives
            )
        };
        match report {
            MusicReport::Mixture(r) => {
                lines = vec![self.t(Key::MuscriptorDraft).into()];
                for n in r.analysis.notes.iter().take(256) {
                    lines.push(
                        format!(
                            "{}: {} / {:.2}–{:.2} s",
                            n.instrument,
                            n.pitch,
                            n.start + r.source_offset_seconds,
                            n.end + r.source_offset_seconds
                        )
                        .into(),
                    );
                }
                if r.analysis.notes.is_empty() {
                    lines.push(self.t(Key::AnalysisUnknown).into());
                }
                if r.analysis.notes.len() > 256 {
                    lines.push(self.t(Key::AnalysisPreviewLimit).into());
                }
            }
            MusicReport::Instruments(r) => {
                lines = vec![
                    self.t(Key::AnalysisInstruments).into(),
                    self.t(Key::AnalysisInstrumentMean).into(),
                ];
                let candidates = |items: &[auris_session::InstrumentCandidate]| {
                    if items.is_empty() {
                        self.t(Key::AnalysisUnknown).to_string()
                    } else {
                        items
                            .iter()
                            .map(|c| format!("{} ({:.2})", c.label, c.score))
                            .collect::<Vec<_>>()
                            .join(" / ")
                    }
                };
                lines.push(candidates(&r.analysis.candidates).into());
                for w in r.analysis.windows.iter().take(256) {
                    lines.push(
                        format!(
                            "{:.2}–{:.2} s: {}",
                            w.start + r.source_offset_seconds,
                            w.end + r.source_offset_seconds,
                            candidates(&w.candidates)
                        )
                        .into(),
                    );
                }
                if r.analysis.windows.len() > 256 {
                    lines.push(self.t(Key::AnalysisPreviewLimit).into());
                }
            }
            MusicReport::Chords(r) => {
                lines.push(self.t(Key::AnalysisBeatUnits).into());
                lines.extend(r.segments.iter().take(256).map(|s| {
                    format!(
                        "{:.2}–{:.2}: {}",
                        s.start.raw() as f64 / 960.0,
                        s.end.raw() as f64 / 960.0,
                        reading(&s.reading)
                    )
                    .into()
                }));
                if r.segments.len() > 256 {
                    lines.push(self.t(Key::AnalysisPreviewLimit).into());
                }
            }
            MusicReport::Audio(r) => {
                lines.push(self.t(Key::AnalysisSecondUnits).into());
                if r.analysis.tempo.candidates.is_empty() {
                    lines.push(self.t(Key::AnalysisNoTempo).into());
                }
                for c in &r.analysis.tempo.candidates {
                    lines.push(format!("{:.1} BPM ({:.2})", c.bpm, c.score).into());
                }
                if r.analysis.options.transcribe {
                    lines.push(self.t(Key::AnalysisMonophonic).into());
                    lines.extend(r.analysis.notes.iter().take(256).map(|n| {
                        format!(
                            "{}: {:.2}–{:.2} s",
                            n.pitch,
                            n.start + r.source_offset_seconds,
                            n.end + r.source_offset_seconds
                        )
                        .into()
                    }));
                    if r.analysis.notes.len() > 256 {
                        lines.push(self.t(Key::AnalysisPreviewLimit).into());
                    }
                }
                lines.extend(r.analysis.chords.iter().take(256).map(|s| {
                    format!(
                        "{:.2}–{:.2} s: {}",
                        s.start + r.source_offset_seconds,
                        s.end + r.source_offset_seconds,
                        reading(&s.reading)
                    )
                    .into()
                }));
                if r.analysis.chords.len() > 256 {
                    lines.push(self.t(Key::AnalysisPreviewLimit).into());
                }
            }
        }
        self.open_prompt(Prompt::notice(self.t(Key::AnalysisTitle), lines));
    }

    /// Renders worker progress and the last report's acceptance controls.
    pub(crate) fn music_analysis_rows(&self, cx: &mut Context<Self>) -> Vec<AnyElement> {
        let theme = self.theme.clone();
        let mut rows = Vec::new();
        let action_button = |id: &'static str, label: String, action: MenuCommand| {
            button(
                id,
                label,
                ButtonStyle::Normal,
                false,
                theme.accent,
                &theme,
                cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                    this.run_menu_command(action.clone(), cx);
                    cx.notify();
                }),
            )
            .into_any_element()
        };
        rows.push(action_button(
            "music-all",
            self.t(Key::MenuAnalyzeAllChords).to_string(),
            MenuCommand::AnalyzeChords(None),
        ));
        if let Some(control) = &self.music_analysis.control {
            rows.push(
                div()
                    .text_xs()
                    .child(format!(
                        "{} {:.0}%",
                        self.t(Key::AnalysisRunning),
                        control.progress() * 100.0
                    ))
                    .into_any_element(),
            );
            rows.push(action_button(
                "music-cancel",
                self.t(Key::AnalysisCancel).to_string(),
                MenuCommand::CancelMusicAnalysis,
            ));
        }
        let Some(report) = &self.music_analysis.report else {
            return rows;
        };
        rows.push(action_button(
            "music-view",
            self.t(Key::AnalysisView).to_string(),
            MenuCommand::ViewMusicAnalysis,
        ));
        // Validation is performed on completion and application, not while painting every frame.
        if matches!(report, MusicReport::Instruments(_)) {
            return rows;
        }
        if let MusicReport::Mixture(r) = report {
            if !r.analysis.notes.is_empty() {
                rows.push(action_button(
                    "music-mixture-notes",
                    self.t(Key::MuscriptorCreateTracks).to_string(),
                    MenuCommand::CreateTranscriptionTrack,
                ));
            }
            return rows;
        }
        rows.push(action_button(
            "music-apply",
            self.t(Key::AnalysisApplyChords).to_string(),
            MenuCommand::ApplyMusicChords,
        ));
        if let MusicReport::Audio(r) = report {
            if !r.analysis.notes.is_empty() {
                rows.push(action_button(
                    "music-notes",
                    self.t(Key::AnalysisCreateNotes).to_string(),
                    MenuCommand::CreateTranscriptionTrack,
                ));
            }
            for (i, c) in r.analysis.tempo.candidates.iter().enumerate() {
                rows.push(
                    button(
                        ("music-tempo", i),
                        format!("{}: {:.1}", self.t(Key::AnalysisSetSourceTempo), c.bpm),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                            this.run_menu_command(MenuCommand::ApplyMusicTempo(i), cx);
                            cx.notify();
                        }),
                    )
                    .into_any_element(),
                );
            }
        }
        if let MusicReport::Chords(r) = report {
            rows.push(
                div()
                    .text_xs()
                    .child(self.t(Key::AnalysisBeatUnits))
                    .into_any_element(),
            );
            for (i, s) in r.segments.iter().take(256).enumerate() {
                rows.push(
                    div()
                        .text_xs()
                        .child(format!(
                            "{:.2}–{:.2}{}",
                            s.start.raw() as f64 / 960.0,
                            s.end.raw() as f64 / 960.0,
                            if s.reading.state == ChordState::Unknown {
                                format!(" · {}", self.t(Key::AnalysisUnknown))
                            } else {
                                String::new()
                            }
                        ))
                        .into_any_element(),
                );
                for (j, c) in s.reading.candidates.iter().enumerate() {
                    rows.push(
                        button(
                            ("music-candidate", i * 4 + j),
                            format!("{} ({:.2})", c.symbol, c.score),
                            ButtonStyle::Normal,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _: &gpui::ClickEvent, _, cx| {
                                this.run_menu_command(
                                    MenuCommand::ApplyMusicCandidate {
                                        segment: i,
                                        candidate: j,
                                    },
                                    cx,
                                );
                                cx.notify();
                            }),
                        )
                        .into_any_element(),
                    );
                }
            }
            if r.segments.len() > 256 {
                rows.push(
                    div()
                        .text_xs()
                        .child(self.t(Key::AnalysisPreviewLimit))
                        .into_any_element(),
                );
            }
        }
        rows
    }
}
