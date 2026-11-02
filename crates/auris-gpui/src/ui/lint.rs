//! The music linter in the editor: the Problems tab, the marks on the lanes, and the two
//! schedules that keep both current.
//!
//! The score rules are cheap and re-run shortly after every edit. The audio rules render the
//! song, so they run on a worker, and only when all of these hold: the background check is on,
//! the document has been *edited* (opening a project is not an edit, and an untouched song is
//! never rendered again and again), it has then held still for [`AUDIO_DEBOUNCE`], and nothing
//! else is using the machine — no playback, recording, export, singing or agent turn. A project
//! with hosted plugins is checked only when asked, because building its render instantiates
//! somebody else's plugins on this thread.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use auris_i18n::Key;
use auris_session::lint::{
    AudioLintResult, Diagnostic, Fix, LintReport, LintScope, LintSuppression, Rule, Severity,
};
use gpui::{AnyElement, Context, IntoElement, Pixels, Point, div, prelude::*, px};

use crate::app::AurisApp;
use crate::theme::Theme;
use crate::ui::context_menu::{ContextMenu, MenuCommand};
use crate::ui::widgets::{ButtonState, ButtonStyle, button, button_enabled};

/// How long the score must hold still before the score rules run again.
pub const SCORE_DEBOUNCE: Duration = Duration::from_millis(400);
/// How long the document must hold still after an edit before the song is rendered.
pub const AUDIO_DEBOUNCE: Duration = Duration::from_secs(4);
/// The most marks drawn on the lanes; the list holds the rest.
const MAX_MARKS: usize = 200;

/// Which half of the bottom panel is showing.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum LogTab {
    /// The linter's findings.
    Problems,
    /// What the application logged. First by default, because the panel is where a warning
    /// somebody was told to look for is; the Problems tab says its count in the header.
    #[default]
    Log,
}

/// Everything the editor keeps about the linter.
#[derive(Default)]
pub struct LintState {
    /// The last score lint, with any audio findings that are still current merged in.
    pub report: LintReport,
    /// Whether [`Self::report`] has been computed at all.
    pub ready: bool,
    /// The revision last seen, and when it was first seen.
    pub seen: Option<(u64, Instant)>,
    /// The latest audio pass's findings and the revision they measured.
    pub audio: Option<AudioLintResult>,
    /// An audio pass in flight, and the revision it renders.
    pub running: Option<(u64, Arc<AtomicBool>)>,
    /// Whether an edit has happened that the audio has not been checked since.
    pub audio_wanted: bool,
    /// The last audio failure, shown until the next pass.
    pub audio_error: Option<String>,
    /// Which tab of the bottom panel is up.
    pub tab: LogTab,
    /// The problem last revealed from the list, drawn emphasised on the lanes.
    pub focused: Option<String>,
    /// Whether the Problems tab lists the project's ignore rules.
    pub show_suppressions: bool,
}

impl LintState {
    /// The findings to show: the score lint plus the audio lint when it measured this revision.
    pub fn diagnostics(&self) -> &[Diagnostic] {
        &self.report.diagnostics
    }

    fn rebuild(&mut self, mut report: LintReport, project: &auris_session::prelude::Project) {
        if let Some(audio) = &self.audio
            && audio.revision == report.revision
        {
            report.merge_audio(audio.clone(), project);
        }
        self.report = report;
        self.ready = true;
    }
}

/// The colour a severity is drawn in.
pub fn severity_colour(severity: Severity, theme: &Theme) -> gpui::Hsla {
    match severity {
        Severity::Error => theme.danger,
        Severity::Warning => theme.warning,
        Severity::Info => theme.accent,
    }
}

fn number(value: f32) -> String {
    if (value - value.round()).abs() < 0.05 {
        format!("{value:.0}")
    } else {
        format!("{value:.1}")
    }
}

impl AurisApp {
    /// Re-lints after edits and starts the background audio pass when the machine is idle.
    /// Called from the repaint timer.
    pub(crate) fn poll_lint(&mut self, cx: &mut Context<Self>) {
        if self.reference_editor {
            return;
        }
        let revision = self.session.revision();
        let now = Instant::now();
        match self.lint.seen {
            Some((seen, _)) if seen == revision => {}
            Some(_) => {
                self.lint.seen = Some((revision, now));
                if self.session.is_dirty() {
                    self.lint.audio_wanted = true;
                }
                // An edit outdates a render in flight; its answer would be about another song.
                if let Some((_, cancel)) = &self.lint.running
                    && self.lint.running.as_ref().map(|(r, _)| *r) != Some(revision)
                {
                    cancel.store(true, Ordering::Relaxed);
                }
                return;
            }
            None => {
                // The document as it was opened: lint it, but it was not an edit.
                self.lint.seen = Some((revision, now));
            }
        }
        let held = self.lint.seen.map_or(Duration::ZERO, |(_, at)| now - at);
        if (self.lint.report.revision != revision || !self.lint.ready)
            && (held >= SCORE_DEBOUNCE || !self.lint.ready)
            && self.drag.is_none()
        {
            let report = self.session.lint_score();
            self.lint.rebuild(report, self.session.project());
            cx.notify();
        }
        if self.settings.background_lint
            && self.lint.audio_wanted
            && held >= AUDIO_DEBOUNCE
            && self.lint_machine_idle()
            && !self.session.has_hosted_plugins()
        {
            self.start_audio_lint(cx);
        }
    }

    /// Nothing else is using the machine: no transport, no render, no voice, no agent turn.
    fn lint_machine_idle(&self) -> bool {
        self.lint.running.is_none()
            && self.drag.is_none()
            && !self.is_playing()
            && !self.session.is_recording()
            && !self.choosing_export
            && self.export.as_ref().is_none_or(|e| e.result.is_some())
            && self.auto_sing.is_none()
            && !self.agent_chat.busy
            && !self.agent_vocal_busy()
            && std::thread::available_parallelism().is_ok_and(|n| n.get() >= 2)
    }

    /// Renders and measures the song on a worker; the result is kept only if nothing changed.
    pub(crate) fn start_audio_lint(&mut self, cx: &mut Context<Self>) {
        if self.lint.running.is_some() {
            return;
        }
        self.lint.audio_wanted = false;
        self.lint.audio_error = None;
        let job = self.session.audio_lint_job();
        let revision = job.revision();
        let cancel = Arc::new(AtomicBool::new(false));
        self.lint.running = Some((revision, Arc::clone(&cancel)));
        cx.notify();
        cx.spawn(async move |this, cx| {
            let flag = Arc::clone(&cancel);
            let result = cx
                .background_executor()
                .spawn(async move { job.run(&flag) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let ours = this
                    .lint
                    .running
                    .as_ref()
                    .is_some_and(|(_, running)| Arc::ptr_eq(running, &cancel));
                if !ours {
                    return;
                }
                // The revision the run is filed under, which ignoring a problem moves forward
                // without changing a sample of what is being rendered.
                let started = this.lint.running.take().map(|(revision, _)| revision);
                let current = this.session.revision();
                match result {
                    Ok(mut result) if started == Some(current) => {
                        result.revision = current;
                        this.lint.audio = Some(result);
                        let report = this.session.lint_score();
                        this.lint.rebuild(report, this.session.project());
                    }
                    // An edit landed mid-render: ask again once the document settles.
                    Ok(_) => this.lint.audio_wanted = true,
                    Err(_) if cancel.load(Ordering::Relaxed) => this.lint.audio_wanted = true,
                    Err(error) => {
                        this.lint.audio_error =
                            Some(crate::i18n::error_text(&error, this.language()));
                    }
                }
                cx.notify();
            });
        })
        .detach();
    }

    /// One problem, said in the interface language.
    pub(crate) fn lint_message(&self, diagnostic: &Diagnostic) -> String {
        let key = match diagnostic.rule {
            Rule::CrowdedOnset => Key::LintCrowdedOnset,
            Rule::DenseNotes => Key::LintDenseNotes,
            Rule::HarshInterval => Key::LintHarshInterval,
            Rule::MuddyLowInterval => Key::LintMuddyLowInterval,
            Rule::OutOfKey => Key::LintOutOfKey,
            Rule::AvoidNote => Key::LintAvoidNote,
            Rule::MissingChord => Key::LintMissingChord,
            Rule::DuplicateNote => Key::LintDuplicateNote,
            Rule::InaudibleVelocity => Key::LintInaudibleVelocity,
            Rule::ExtremePitch => Key::LintExtremePitch,
            Rule::Clipping => Key::LintClipping,
            Rule::TooLoud => Key::LintTooLoud,
            Rule::TooQuiet => Key::LintTooQuiet,
            Rule::SilentTrack => Key::LintSilentTrack,
        };
        let pitches = diagnostic
            .pitches
            .iter()
            .map(|pitch| crate::ui::timeline::pitch_name(*pitch))
            .collect::<Vec<_>>()
            .join("・");
        let value = match diagnostic.rule {
            Rule::InaudibleVelocity => format!("{:.2}", diagnostic.value.unwrap_or_default()),
            _ => number(diagnostic.value.unwrap_or_default()),
        };
        self.t(key)
            .replace("{pitches}", &pitches)
            .replace("{value}", &value)
            .replace("{limit}", &number(diagnostic.limit.unwrap_or_default()))
    }

    /// The fix, said as the button's tooltip and the menu row.
    pub(crate) fn lint_fix_label(&self, fix: &Fix) -> String {
        match fix {
            Fix::RemoveNotes { .. } => self.t(Key::LintFixRemove).into(),
            Fix::TrimNotes { .. } => self.t(Key::LintFixTrim).into(),
            Fix::SetPitches { pitches, .. } => self.t(Key::LintFixPitch).replace(
                "{pitch}",
                &pitches
                    .iter()
                    .map(|(_, pitch)| crate::ui::timeline::pitch_name(*pitch))
                    .collect::<Vec<_>>()
                    .join("・"),
            ),
            Fix::SetVelocity { .. } => self.t(Key::LintFixVelocity).into(),
            Fix::ResetFader { .. } => self.t(Key::LintFixFader).into(),
            Fix::NormalizeMix => self.t(Key::LintFixNormalize).into(),
            Fix::SetChords { chords, .. } => {
                let harmony = &self.session.project().harmony;
                let names = chords
                    .iter()
                    .map(|(at, numeral)| numeral.name_in(harmony.key_at(*at)))
                    .collect::<Vec<_>>()
                    .join(" | ");
                self.t(Key::LintFixChords).replace("{chords}", &names)
            }
        }
    }

    /// Where a problem is: its track, or the mix, and its bar.
    pub(crate) fn lint_location(&self, diagnostic: &Diagnostic) -> String {
        let project = self.project();
        let place = diagnostic
            .track
            .and_then(|track| project.track(track))
            .map_or_else(|| self.t(Key::ProblemsMix).to_string(), |t| t.name.clone());
        let bar = project.signatures.bar_of(diagnostic.start);
        format!(
            "{place} · {}",
            self.t(Key::ProblemsLocation)
                .replace("{bar}", &bar.to_string())
        )
    }

    /// Applies one problem's fix, if the problem still stands.
    pub(crate) fn apply_lint(&mut self, id: &str, cx: &mut Context<Self>) {
        let revision = self.session.revision();
        if self.lint.report.revision != revision {
            let report = self.session.lint_score();
            self.lint.rebuild(report, self.session.project());
        }
        let Some(diagnostic) = self.lint.report.find(id).cloned() else {
            self.set_status(self.t(Key::ErrorLintStale));
            cx.notify();
            return;
        };
        match self.session.apply_lint_fix(&diagnostic, revision) {
            Ok(()) => {
                self.set_status(self.t(Key::ProblemsFixed).replace("{count}", "1"));
            }
            Err(error) => {
                let message = crate::i18n::error_text(&error, self.language());
                self.set_failed_status(message);
            }
        }
        self.lint.focused = None;
        let report = self.session.lint_score();
        self.lint.rebuild(report, self.session.project());
        cx.notify();
    }

    /// Applies every score problem's obvious fix as one undo step.
    fn apply_all_lint(&mut self, cx: &mut Context<Self>) {
        let revision = self.session.revision();
        let report = self.session.lint_score();
        self.lint.rebuild(report, self.session.project());
        // A mix normalization renders, so "fix everything" leaves it to its own button.
        let fixable: Vec<&Diagnostic> = self
            .lint
            .report
            .diagnostics
            .iter()
            .filter(|d| d.fix.as_ref().is_some_and(|fix| !fix.renders()))
            .collect();
        match self.session.apply_lint_fixes(&fixable, revision) {
            Ok(count) => {
                self.set_status(
                    self.t(Key::ProblemsFixed)
                        .replace("{count}", &count.to_string()),
                );
            }
            Err(error) => {
                let message = crate::i18n::error_text(&error, self.language());
                self.set_failed_status(message);
            }
        }
        let report = self.session.lint_score();
        self.lint.rebuild(report, self.session.project());
        cx.notify();
    }

    /// Hands one problem to the agent as an ordinary message, opening its panel.
    pub(crate) fn ask_agent_about_lint(&mut self, id: &str, cx: &mut Context<Self>) {
        let Some(diagnostic) = self.lint.report.find(id).cloned() else {
            return;
        };
        let text = self
            .t(Key::ProblemsAgentPrompt)
            .replace("{problem}", &self.lint_message(&diagnostic))
            .replace("{where}", &self.lint_location(&diagnostic))
            .replace("{rule}", diagnostic.rule.name())
            .replace("{id}", &diagnostic.id())
            .replace("{start}", &diagnostic.start.raw().to_string())
            .replace("{end}", &diagnostic.end.raw().to_string());
        self.show_panel(crate::dock::Panel::Agent);
        let length = self.agent_chat.input.content().len();
        self.agent_chat.input.replace(0..length, &text);
        self.agent_send();
        cx.notify();
    }

    /// Whether an agent could be asked: a model is configured.
    pub(crate) fn lint_agent_available(&self) -> bool {
        self.settings.agent.is_configured()
    }

    /// Selects the problem's track and clip and scrolls the lanes to it.
    pub(crate) fn reveal_lint(&mut self, id: &str) {
        let Some(diagnostic) = self.lint.report.find(id).cloned() else {
            return;
        };
        if let Some(track) = diagnostic.track {
            self.select_track(track);
        }
        if let Some(note) = diagnostic.notes.first() {
            self.select_clip(Some(note.clip));
            self.selected_notes = diagnostic
                .notes
                .iter()
                .filter(|n| n.clip == note.clip)
                .map(|n| n.index)
                .collect();
        }
        let width = self.arrangement_width;
        self.timeline.scroll_to_reveal(diagnostic.start, width);
        self.lint.focused = Some(id.to_string());
    }

    /// The menu a mark on the lanes opens: what is wrong, and what can be done about it.
    pub(crate) fn lint_menu(&self, anchor: Point<Pixels>, id: &str) -> ContextMenu {
        let Some(diagnostic) = self.lint.report.find(id) else {
            return ContextMenu::new(anchor, self.t(Key::ProblemsTab));
        };
        let mut menu = ContextMenu::new(anchor, self.lint_message(diagnostic));
        if let Some(fix) = &diagnostic.fix {
            menu = menu.item(
                format!("{}: {}", self.t(Key::ProblemsFix), self.lint_fix_label(fix)),
                MenuCommand::LintFix(id.to_string()),
            );
        }
        let mut menu = menu
            .item_if(
                self.lint_agent_available(),
                self.t(Key::ProblemsAskAgent),
                MenuCommand::LintAskAgent(id.to_string()),
            )
            .item(
                self.t(Key::ProblemsShowInPanel),
                MenuCommand::LintShow(id.to_string()),
            )
            .separator();
        for (label, suppression) in self.lint_ignore_options(diagnostic) {
            menu = menu.item(label, MenuCommand::LintIgnore(suppression));
        }
        menu
    }

    /// The menu behind a problem row's Ignore button.
    pub(crate) fn lint_ignore_menu(&self, anchor: Point<Pixels>, id: &str) -> ContextMenu {
        let mut menu = ContextMenu::new(anchor, self.t(Key::ProblemsIgnore));
        if let Some(diagnostic) = self.lint.report.find(id) {
            for (label, suppression) in self.lint_ignore_options(diagnostic) {
                menu = menu.item(label, MenuCommand::LintIgnore(suppression));
            }
        }
        menu
    }

    /// A rule's short name, in the interface language.
    pub(crate) fn lint_rule_name(&self, rule: Rule) -> &'static str {
        self.t(match rule {
            Rule::CrowdedOnset => Key::LintNameCrowdedOnset,
            Rule::DenseNotes => Key::LintNameDenseNotes,
            Rule::HarshInterval => Key::LintNameHarshInterval,
            Rule::MuddyLowInterval => Key::LintNameMuddyLowInterval,
            Rule::OutOfKey => Key::LintNameOutOfKey,
            Rule::AvoidNote => Key::LintNameAvoidNote,
            Rule::MissingChord => Key::LintNameMissingChord,
            Rule::DuplicateNote => Key::LintNameDuplicateNote,
            Rule::InaudibleVelocity => Key::LintNameInaudibleVelocity,
            Rule::ExtremePitch => Key::LintNameExtremePitch,
            Rule::Clipping => Key::LintNameClipping,
            Rule::TooLoud => Key::LintNameTooLoud,
            Rule::TooQuiet => Key::LintNameTooQuiet,
            Rule::SilentTrack => Key::LintNameSilentTrack,
        })
    }

    fn lint_track_label(&self, track: auris_session::prelude::TrackId) -> String {
        self.session.project().track(track).map_or_else(
            || self.t(Key::ProblemsDeleted).to_string(),
            |t| t.name.clone(),
        )
    }

    fn lint_clip_label(&self, clip: auris_session::prelude::ClipId) -> String {
        match self.clip_name(clip) {
            Some(name) if !name.is_empty() => format!("“{name}”"),
            Some(_) => self.t(Key::ProblemsThisClip).to_string(),
            None => self.t(Key::ProblemsDeleted).to_string(),
        }
    }

    /// Every way of ignoring `diagnostic`, narrowest last within each kind: this rule
    /// everywhere, on its track, in its clip, in its section, then every rule on the same three.
    pub(crate) fn lint_ignore_options(
        &self,
        diagnostic: &Diagnostic,
    ) -> Vec<(String, LintSuppression)> {
        let rule_name = self.lint_rule_name(diagnostic.rule);
        let wire = Some(diagnostic.rule.name().to_string());
        let project = self.session.project();
        let mut clips: Vec<_> = diagnostic.notes.iter().map(|note| note.clip).collect();
        clips.dedup();
        let clip = (clips.len() == 1).then(|| clips[0]);
        let section = project
            .sections
            .section_at(diagnostic.start)
            .map(|(label, _)| label.to_string());
        let rule = |key: Key| self.t(key).replace("{rule}", rule_name);
        let mut options = vec![(
            rule(Key::ProblemsIgnoreRuleEverywhere),
            LintSuppression {
                rule: wire.clone(),
                scope: LintScope::Song,
            },
        )];
        let mut wide = Vec::new();
        if let Some(track) = diagnostic.track {
            let name = self.lint_track_label(track);
            options.push((
                rule(Key::ProblemsIgnoreRuleTrack).replace("{track}", &name),
                LintSuppression {
                    rule: wire.clone(),
                    scope: LintScope::Track { track },
                },
            ));
            wide.push((
                self.t(Key::ProblemsIgnoreTrack).replace("{track}", &name),
                LintSuppression {
                    rule: None,
                    scope: LintScope::Track { track },
                },
            ));
        }
        if let Some(clip) = clip {
            let name = self.lint_clip_label(clip);
            options.push((
                rule(Key::ProblemsIgnoreRuleClip).replace("{clip}", &name),
                LintSuppression {
                    rule: wire.clone(),
                    scope: LintScope::Clip { clip },
                },
            ));
            wide.push((
                self.t(Key::ProblemsIgnoreClip).replace("{clip}", &name),
                LintSuppression {
                    rule: None,
                    scope: LintScope::Clip { clip },
                },
            ));
        }
        if let Some(label) = section {
            options.push((
                rule(Key::ProblemsIgnoreRuleSection).replace("{section}", &label),
                LintSuppression {
                    rule: wire,
                    scope: LintScope::Section {
                        label: label.clone(),
                    },
                },
            ));
            wide.push((
                self.t(Key::ProblemsIgnoreSection)
                    .replace("{section}", &label),
                LintSuppression {
                    rule: None,
                    scope: LintScope::Section { label },
                },
            ));
        }
        options.extend(wide);
        options
    }

    /// One ignore rule, said as a row of the list.
    pub(crate) fn lint_suppression_label(&self, suppression: &LintSuppression) -> String {
        let rule = match suppression.rule.as_deref() {
            None => self.t(Key::ProblemsEveryRule).to_string(),
            Some(name) => Rule::ALL
                .iter()
                .find(|rule| rule.name() == name)
                .map_or_else(
                    || name.to_string(),
                    |rule| self.lint_rule_name(*rule).into(),
                ),
        };
        let scope = match &suppression.scope {
            LintScope::Song => self.t(Key::ProblemsScopeSong).to_string(),
            LintScope::Track { track } => self
                .t(Key::ProblemsScopeTrack)
                .replace("{track}", &self.lint_track_label(*track)),
            LintScope::Clip { clip } => self
                .t(Key::ProblemsScopeClip)
                .replace("{clip}", &self.lint_clip_label(*clip)),
            LintScope::Section { label } => self
                .t(Key::ProblemsScopeSection)
                .replace("{section}", label),
        };
        format!("{rule}: {scope}")
    }

    /// Adds an ignore rule to the project and re-lints at once.
    pub(crate) fn lint_ignore(&mut self, suppression: LintSuppression, cx: &mut Context<Self>) {
        let before = self.session.revision();
        self.session.suppress_lint(suppression);
        self.lint_suppressions_changed(before);
        cx.notify();
    }

    /// Removes an ignore rule and re-lints at once.
    pub(crate) fn lint_unignore(&mut self, suppression: &LintSuppression, cx: &mut Context<Self>) {
        let before = self.session.revision();
        self.session.unsuppress_lint(suppression);
        self.lint_suppressions_changed(before);
        cx.notify();
    }

    /// What ignoring changes is which findings are shown, not the sound: the audio pass of the
    /// revision before still holds, and this edit is no reason to render the song again.
    fn lint_suppressions_changed(&mut self, before: u64) {
        let now = self.session.revision();
        if now == before {
            return;
        }
        if let Some(audio) = self.lint.audio.as_mut()
            && audio.revision == before
        {
            audio.revision = now;
        }
        if self.lint.seen.is_some_and(|(seen, _)| seen == before) {
            self.lint.seen = Some((now, Instant::now()));
        }
        if let Some((revision, _)) = self.lint.running.as_mut()
            && *revision == before
        {
            *revision = now;
        }
        self.lint.focused = None;
        let report = self.session.lint_score();
        self.lint.rebuild(report, self.session.project());
    }

    /// The Problems tab.
    pub(crate) fn render_problems(&mut self, cx: &mut Context<Self>) -> AnyElement {
        let theme = self.theme.clone();
        let (errors, warnings, infos) = self.lint.report.counts();
        let summary = self
            .t(Key::ProblemsSummary)
            .replace("{errors}", &errors.to_string())
            .replace("{warnings}", &warnings.to_string())
            .replace("{infos}", &infos.to_string());
        let running = self.lint.running.is_some();
        let fixable = self
            .lint
            .report
            .diagnostics
            .iter()
            .any(|d| d.fix.as_ref().is_some_and(|fix| !fix.renders()));
        let audio_line = if running {
            Some((self.t(Key::ProblemsAnalyzing).to_string(), theme.text_muted))
        } else if let Some(error) = &self.lint.audio_error {
            Some((
                self.t(Key::ProblemsAudioFailed).replace("{error}", error),
                theme.danger,
            ))
        } else if let Some(summary) = self.lint.report.audio.as_ref() {
            Some((
                match summary.integrated_lufs {
                    Some(lufs) => self
                        .t(Key::ProblemsAudioSummary)
                        .replace("{lufs}", &format!("{lufs:.1}"))
                        .replace("{peak}", &format!("{:.1}", summary.peak_db)),
                    None => self.t(Key::ProblemsAudioSilent).to_string(),
                },
                theme.text_muted,
            ))
        } else if self.session.has_hosted_plugins() {
            Some((
                self.t(Key::ProblemsHostedManual).to_string(),
                theme.text_faint,
            ))
        } else {
            Some((
                self.t(Key::ProblemsAudioPending).to_string(),
                theme.text_faint,
            ))
        };
        let agent = self.lint_agent_available();
        let rows: Vec<AnyElement> = self
            .lint
            .report
            .diagnostics
            .iter()
            .enumerate()
            .map(|(index, diagnostic)| self.problem_row(index, diagnostic, agent, &theme, cx))
            .collect();
        let empty = rows.is_empty();
        let suppressions = self.session.project().lint_suppressions.clone();
        let show_suppressions = self.lint.show_suppressions && !suppressions.is_empty();
        let suppression_rows: Vec<AnyElement> = if show_suppressions {
            suppressions
                .iter()
                .enumerate()
                .map(|(index, suppression)| {
                    let removed = suppression.clone();
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_1p5()
                        .py_0p5()
                        .text_xs()
                        .debug_selector(move || format!("problems-ignore-rule-{index}"))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(theme.text_muted)
                                .child(self.lint_suppression_label(suppression)),
                        )
                        .child(button(
                            ("problems-unignore", index),
                            self.t(Key::ProblemsUnignore),
                            ButtonStyle::Ghost,
                            false,
                            theme.accent,
                            &theme,
                            cx.listener(move |this, _, _, cx| this.lint_unignore(&removed, cx)),
                        ))
                        .into_any_element()
                })
                .collect()
        } else {
            Vec::new()
        };
        let ignored = (self.lint.report.suppressed > 0).then(|| {
            self.t(Key::ProblemsIgnored)
                .replace("{count}", &self.lint.report.suppressed.to_string())
        });
        div()
            .flex()
            .flex_col()
            .flex_1()
            .min_h_0()
            .min_w_0()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .px_2()
                    .py_1()
                    .border_b_1()
                    .border_color(theme.border_subtle)
                    .text_xs()
                    .child(
                        div()
                            .text_color(theme.text_muted)
                            .debug_selector(|| "problems-summary".to_owned())
                            .child(summary),
                    )
                    .when_some(ignored, |this, ignored| {
                        this.child(
                            div()
                                .text_color(theme.text_faint)
                                .debug_selector(|| "problems-ignored".to_owned())
                                .child(ignored),
                        )
                    })
                    .when_some(audio_line, |this, (line, colour)| {
                        this.child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .truncate()
                                .text_color(colour)
                                .child(line),
                        )
                    })
                    .child(button_enabled(
                        "problems-fix-all",
                        self.t(Key::ProblemsFixAll),
                        ButtonStyle::Normal,
                        ButtonState::available(false, fixable),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.apply_all_lint(cx)),
                    ))
                    .child(button_enabled(
                        "problems-audio",
                        self.t(Key::ProblemsAnalyzeAudio),
                        ButtonStyle::Normal,
                        ButtonState::available(false, !running),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| this.start_audio_lint(cx)),
                    ))
                    .child(
                        button(
                            "problems-background",
                            self.t(Key::ProblemsBackground),
                            ButtonStyle::Ghost,
                            self.settings.background_lint,
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.settings.background_lint = !this.settings.background_lint;
                                if let Err(error) = this.save_settings() {
                                    log::warn!("could not save settings: {error}");
                                }
                                cx.notify();
                            }),
                        )
                        .tooltip(crate::ui::tooltip::keyed_tip(
                            self.t(Key::ProblemsBackgroundTip),
                            "",
                            &theme,
                        )),
                    )
                    .when(!suppressions.is_empty(), |this| {
                        this.child(
                            button(
                                "problems-ignore-rules",
                                self.t(Key::ProblemsIgnoreRules)
                                    .replace("{count}", &suppressions.len().to_string()),
                                ButtonStyle::Ghost,
                                show_suppressions,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.lint.show_suppressions = !this.lint.show_suppressions;
                                    cx.notify();
                                }),
                            )
                            .tooltip(crate::ui::tooltip::keyed_tip(
                                self.t(Key::ProblemsIgnoreRulesTip),
                                "",
                                &theme,
                            )),
                        )
                    }),
            )
            .when(show_suppressions, |this| {
                this.child(
                    div()
                        .flex()
                        .flex_col()
                        .px_1()
                        .py_0p5()
                        .border_b_1()
                        .border_color(theme.border_subtle)
                        .children(suppression_rows),
                )
            })
            .child(
                div()
                    .id("problems-rows")
                    .debug_selector(|| "problems-rows".to_owned())
                    .flex()
                    .flex_col()
                    .flex_1()
                    .min_h_0()
                    .p_1()
                    .overflow_y_scroll()
                    .children(rows)
                    .when(empty, |this| {
                        this.child(
                            div()
                                .p_2()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .child(self.t(Key::ProblemsEmpty)),
                        )
                    }),
            )
            .into_any_element()
    }

    fn problem_row(
        &self,
        index: usize,
        diagnostic: &Diagnostic,
        agent: bool,
        theme: &Theme,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let id = diagnostic.id();
        let severity = match diagnostic.severity {
            Severity::Error => Key::LintSeverityError,
            Severity::Warning => Key::LintSeverityWarning,
            Severity::Info => Key::LintSeverityInfo,
        };
        let focused = self.lint.focused.as_deref() == Some(id.as_str());
        let reveal = id.clone();
        let fix_id = id.clone();
        let ask_id = id.clone();
        let ignore_id = id.clone();
        div()
            .id(("problem", index))
            .debug_selector(move || format!("problem-{index}"))
            .flex()
            .items_center()
            .gap_2()
            .px_1p5()
            .py_0p5()
            .rounded_sm()
            .text_xs()
            .when(focused, |this| this.bg(theme.surface_raised))
            .hover(|style| style.bg(theme.surface_raised))
            .cursor_pointer()
            .on_click(cx.listener(move |this, _, _, cx| {
                this.reveal_lint(&reveal);
                cx.notify();
            }))
            .child(
                div()
                    .w(px(56.0))
                    .flex_shrink_0()
                    .text_color(severity_colour(diagnostic.severity, theme))
                    .child(self.t(severity)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .whitespace_normal()
                    .text_color(theme.text)
                    .child(self.lint_message(diagnostic)),
            )
            .child(
                div()
                    .flex_shrink_0()
                    .text_color(theme.text_faint)
                    .child(self.lint_location(diagnostic)),
            )
            .when_some(diagnostic.fix.as_ref(), |this, fix| {
                this.child(
                    button(
                        ("problem-fix", index),
                        self.t(Key::ProblemsFix),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        theme,
                        cx.listener(move |this, _, _, cx| {
                            cx.stop_propagation();
                            this.apply_lint(&fix_id, cx);
                        }),
                    )
                    .tooltip(crate::ui::tooltip::keyed_tip(
                        self.lint_fix_label(fix),
                        "",
                        theme,
                    )),
                )
            })
            .child(button(
                ("problem-ignore", index),
                self.t(Key::ProblemsIgnore),
                ButtonStyle::Ghost,
                false,
                theme.accent,
                theme,
                AurisApp::opens_menu(cx, move |this, at| this.lint_ignore_menu(at, &ignore_id)),
            ))
            .when(agent, |this| {
                this.child(button(
                    ("problem-ask", index),
                    self.t(Key::ProblemsAskAgent),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        cx.stop_propagation();
                        this.ask_agent_about_lint(&ask_id, cx);
                    }),
                ))
            })
            .into_any_element()
    }

    /// The marks drawn over the lanes, one per problem, positioned in the lanes' own
    /// coordinates: `(id, severity, left, top, width, emphasised)`.
    pub(crate) fn lint_marks(&self) -> Vec<LintMark> {
        let rows = self.lane_rows();
        let total = crate::ui::automation::rows_height(&rows);
        self.lint
            .report
            .diagnostics
            .iter()
            .take(MAX_MARKS)
            .filter_map(|diagnostic| {
                let (top, height) = match diagnostic.track {
                    Some(track) => {
                        let row = rows.iter().find(|row| {
                            row.track == track
                                && matches!(row.kind, crate::ui::automation::RowKind::Clips)
                        })?;
                        (row.top, row.height)
                    }
                    None => (px(0.0), total),
                };
                let left = self.timeline.tick_to_x(diagnostic.start);
                let width = self
                    .timeline
                    .duration_to_width(diagnostic.end - diagnostic.start)
                    .max(px(6.0));
                Some(LintMark {
                    id: diagnostic.id(),
                    message: self.lint_message(diagnostic),
                    severity: diagnostic.severity,
                    left,
                    top,
                    height,
                    width,
                    focused: self.lint.focused.as_deref() == Some(diagnostic.id().as_str()),
                    mix: diagnostic.track.is_none(),
                })
            })
            .collect()
    }
}

/// One mark on the lanes.
#[derive(Clone, Debug)]
pub struct LintMark {
    /// The problem's id.
    pub id: String,
    /// What the hover says.
    pub message: String,
    /// How it is coloured.
    pub severity: Severity,
    /// Left edge, in the lanes' coordinates before vertical scrolling.
    pub left: Pixels,
    /// Top of the row the problem is on.
    pub top: Pixels,
    /// Height of that row, or of every row for a mix-wide problem.
    pub height: Pixels,
    /// How wide the problem is on the timeline.
    pub width: Pixels,
    /// Whether it was just revealed from the list.
    pub focused: bool,
    /// Whether it belongs to the whole mix rather than a track.
    pub mix: bool,
}

/// How tall the hover strip at the foot of a lane is.
pub const MARK_STRIP: Pixels = px(5.0);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dock::Panel;
    use crate::harness;
    use auris_session::prelude::{Note, Ticks};
    use gpui::{TestAppContext, size};

    /// A clip holding one note written twice, linted, with the Problems tab up.
    fn with_a_duplicate(
        cx: &mut TestAppContext,
    ) -> (
        gpui::Entity<AurisApp>,
        &mut gpui::VisualTestContext,
        auris_session::prelude::TrackId,
    ) {
        let (app, cx, track, clip) = harness::with_a_clip(cx);
        app.update(cx, |this, cx| {
            this.session
                .replace_notes(
                    clip,
                    vec![
                        Note::new(60, Ticks::ZERO, Ticks(480)),
                        Note::new(60, Ticks::ZERO, Ticks(480)),
                    ],
                )
                .unwrap();
            this.poll_lint(cx);
            this.lint.tab = LogTab::Problems;
            this.show_panel(Panel::Log);
        });
        harness::resize(&app, cx, size(px(1200.0), px(800.0)));
        (app, cx, track)
    }

    #[gpui::test]
    fn a_problem_ignored_on_its_track_is_saved_in_the_project_and_can_be_shown_again(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track) = with_a_duplicate(cx);
        assert!(cx.debug_bounds("problem-0").is_some());
        assert!(cx.debug_bounds("problems-ignore-rules").is_none());

        harness::click("problem-ignore-0", cx);
        harness::choose(
            &app,
            cx,
            &MenuCommand::LintIgnore(LintSuppression {
                rule: Some(Rule::DuplicateNote.name().into()),
                scope: LintScope::Track { track },
            }),
        );
        harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert!(this.lint.report.diagnostics.is_empty());
            assert_eq!(this.lint.report.suppressed, 1);
            assert_eq!(this.session.project().lint_suppressions.len(), 1);
        });
        // Not asked of `debug_bounds` that the row has gone: gpui keeps a selector's last
        // bounds after the element stops being drawn, so only an appearance can be asserted.
        assert!(cx.debug_bounds("problems-ignored").is_some());

        harness::click("problems-ignore-rules", cx);
        harness::paint(&app, cx);
        harness::click("problems-unignore-0", cx);
        harness::paint(&app, cx);
        app.read_with(cx, |this, _| {
            assert_eq!(this.lint.report.diagnostics.len(), 1);
            assert!(this.session.project().lint_suppressions.is_empty());
        });
    }

    #[gpui::test]
    fn the_ignore_menu_offers_the_rule_and_every_rule_for_each_scope_the_problem_has(
        cx: &mut TestAppContext,
    ) {
        let (app, cx, track) = with_a_duplicate(cx);
        app.update(cx, |this, _| {
            this.session.set_section(Ticks::ZERO, Some("Verse".into()));
        });
        app.read_with(cx, |this, _| {
            let report = this.session.lint_score();
            let options: Vec<_> = this
                .lint_ignore_options(&report.diagnostics[0])
                .into_iter()
                .map(|(_, suppression)| suppression)
                .collect();
            let rule = Some(Rule::DuplicateNote.name().to_string());
            let clip = report.diagnostics[0].notes[0].clip;
            let verse = LintScope::Section {
                label: "Verse".into(),
            };
            let expected: Vec<_> = [
                (rule.clone(), LintScope::Song),
                (rule.clone(), LintScope::Track { track }),
                (rule.clone(), LintScope::Clip { clip }),
                (rule, verse.clone()),
                (None, LintScope::Track { track }),
                (None, LintScope::Clip { clip }),
                (None, verse),
            ]
            .into_iter()
            .map(|(rule, scope)| LintSuppression { rule, scope })
            .collect();
            assert_eq!(options, expected);
        });
    }
}
