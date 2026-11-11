//! Local report preparation, exact-excerpt consent and isolated optional AI diagnosis.

use std::{cell::RefCell, rc::Rc, sync::mpsc};

use auris_i18n::{Key, Language};
use auris_session::issue_report::{ContextKind, Draft, Suggestion, bounded, redact};
use gpui::{
    AnyElement, Bounds, Context, FocusHandle, IntoElement, KeyDownEvent, Render, Window,
    WindowBounds, WindowOptions, div, prelude::*, px, size,
};

use crate::ui::{
    text_area::{ProseViewport, editable_scrolling_area},
    text_field::{HasTextField, KeyEffect, TextField},
    widgets::{ButtonState, ButtonStyle, button, button_enabled},
};
use crate::{
    app::AurisApp,
    theme::{Metrics, Theme},
    titlebar,
};

/// Title, description, and one excerpt per [`ContextKind`].
const FIELDS: usize = 2 + ContextKind::ALL.len();

/// How much of a crash report is offered; a backtrace through gpui is long but not this long.
const CRASH_EXCERPT: usize = 60_000;

/// One report window holds consent only for the lifetime of its captured draft.
pub(crate) struct IssueReportWindow {
    theme: Theme,
    language: Language,
    prefs: auris_session::AgentPreferences,
    draft: Draft,
    proposal: Option<Suggestion>,
    fields: [TextField; FIELDS],
    foci: [FocusHandle; FIELDS],
    focus: FocusHandle,
    active: Option<usize>,
    viewports: [Rc<RefCell<ProseViewport>>; FIELDS],
    reviewed: Option<ContextKind>,
    loaded: [bool; ContextKind::ALL.len()],
    /// Whether there is a crash report to offer at all; without one its row is not drawn.
    has_crash: bool,
    context_rx: Option<(ContextKind, mpsc::Receiver<Result<String, String>>)>,
    private_paths: Vec<String>,
    prompt_snapshot: String,
    preview: bool,
    ai_confirmation: Option<String>,
    diagnosis: Option<auris_agent::issue_diagnosis::DiagnosisJob>,
    status: String,
    body_scroll: gpui::ScrollHandle,
}

impl AurisApp {
    /// Open a report invitation's draft; model suggestions never invoke this method directly.
    pub(crate) fn open_issue_report(
        &mut self,
        suggestion: Option<Suggestion>,
        cx: &mut Context<Self>,
    ) {
        self.open_issue_report_with(suggestion, None, cx);
    }

    /// Open a draft about `crash`, with its report already shown for review.
    ///
    /// Shown, not included: the report quotes paths and log lines, so it is held back like every
    /// other excerpt until the user allows it.
    pub(crate) fn open_crash_report(
        &mut self,
        crash: auris_session::diagnostics::CrashReport,
        cx: &mut Context<Self>,
    ) {
        self.open_issue_report_with(None, Some(crash), cx);
    }

    fn open_issue_report_with(
        &mut self,
        suggestion: Option<Suggestion>,
        crash: Option<auris_session::diagnostics::CrashReport>,
        cx: &mut Context<Self>,
    ) {
        // Re-entering the same window must not replace an already reviewed draft or its consent.
        if let Some(handle) = self.issue_report_window
            && handle
                .update(cx, |_, window, _| window.activate_window())
                .is_ok()
        {
            return;
        }
        let audio = self.session.audio_status();
        let environment = format!(
            "Auris Studio {}\nOS: {} ({})\nAudio: {}; {} Hz; {} channels",
            env!("CARGO_PKG_VERSION"),
            std::env::consts::OS,
            std::env::consts::ARCH,
            audio.device,
            audio.sample_rate,
            audio.channels
        );
        let about_crash = crash.is_some();
        // Help → Report a problem offers the newest report too: a crash dismissed at start-up
        // is still the most useful thing a report about it can carry.
        // Not in a test, which would otherwise read the crash folder of whoever runs it.
        let crash = crash.or_else(|| {
            (!cfg!(test))
                .then(|| auris_session::diagnostics::Crashes::default().latest())
                .flatten()
        });
        let title = if about_crash {
            crash
                .as_ref()
                .map(|crash| bounded(&format!("Crash: {}", crash.summary()), 120))
                .unwrap_or_default()
        } else {
            String::new()
        };
        let mut draft = Draft::new(title, self.t(Key::IssueTemplate).to_owned(), environment);
        let prompts = self.issue_prompt_snapshot();
        // The chat panel's editable fields are initialized lazily when it opens.
        let prefs = self.settings.agent.clone();
        let theme = self.theme.clone();
        let language = self.language();
        let project = self
            .session
            .project_folder()
            .map(std::path::Path::to_path_buf);
        let private_paths = with_short_spellings(
            [
                std::env::var("USERPROFILE").ok(),
                std::env::var("HOME").ok(),
                project.as_ref().map(|p| p.display().to_string()),
                Some(auris_session::config_dir().display().to_string()),
                Some(std::env::temp_dir().display().to_string()),
            ]
            .into_iter()
            .flatten()
            .collect(),
        );
        let logs = crate::logbook::book()
            .entries()
            .iter()
            .rev()
            .take(100)
            .map(|e| {
                format!(
                    "{}s {} {}: {}",
                    e.seconds,
                    e.level,
                    e.target,
                    bounded(&e.message, 1000)
                )
            })
            .collect::<Vec<_>>()
            .join("\n");
        let logs = redact(&logs, &private_paths);
        let crash_text = crash
            .as_ref()
            .map(|crash| redact(&bounded(&crash.text, CRASH_EXCERPT), &private_paths));
        let has_crash = crash_text.is_some();
        let environment = redact(&draft.environment, &private_paths);
        draft.environment = environment;
        let bounds = Bounds::centered(None, size(px(760.0), px(760.0)), cx);
        let opened = crate::native_window::open(
            cx,
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                window_min_size: Some(size(px(440.0), px(360.0))),
                focus: true,
                ..titlebar::options(self.t(Key::IssueDraft))
            },
            |_, cx| {
                cx.new(|cx| {
                    let mut fields = std::array::from_fn(|_| TextField::default());
                    fields[0] = TextField::new(draft.title.clone());
                    fields[1] = TextField::new(draft.description.clone());
                    fields[2] = TextField::new(logs);
                    if let Some(text) = crash_text {
                        fields[2 + ContextKind::CrashReport.index()] = TextField::new(text);
                    }
                    IssueReportWindow {
                        theme,
                        language,
                        prefs,
                        draft,
                        proposal: suggestion,
                        fields,
                        foci: std::array::from_fn(|_| cx.focus_handle().tab_stop(true)),
                        focus: cx.focus_handle(),
                        active: Some(0),
                        viewports: std::array::from_fn(|_| {
                            Rc::new(RefCell::new(ProseViewport::default()))
                        }),
                        reviewed: about_crash.then_some(ContextKind::CrashReport),
                        loaded: [true, false, false, has_crash],
                        has_crash,
                        context_rx: None,
                        private_paths,
                        prompt_snapshot: prompts,
                        preview: false,
                        ai_confirmation: None,
                        diagnosis: None,
                        status: String::new(),
                        body_scroll: gpui::ScrollHandle::new(),
                    }
                })
            },
        );
        match opened {
            Ok(handle) => self.issue_report_window = Some(handle),
            Err(error) => self.set_status(error.to_string()),
        }
    }
}

/// The crash report the start screen should offer, once per process and once per report.
///
/// Marked as offered as soon as it is taken, so a report the user dismisses is not offered on
/// every launch after — it stays on disk, and **Help → Report a problem** still attaches it. Once
/// per process because every project window builds its own view, and the offer belongs to the
/// first.
pub(crate) fn take_crash_offer() -> Option<auris_session::diagnostics::CrashReport> {
    static TAKEN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
    if TAKEN.swap(true, std::sync::atomic::Ordering::Relaxed) {
        return None;
    }
    let crashes = auris_session::diagnostics::Crashes::default();
    let report = crashes.unseen()?;
    crashes.mark_seen(&report);
    Some(report)
}

impl AurisApp {
    /// Show the folder holding the log files and crash reports in the system's file manager.
    pub(crate) fn open_log_folder(&mut self, cx: &mut Context<Self>) {
        let dir = auris_session::diagnostics::log_dir();
        // Created first: a launch whose file could not be opened has no folder to show, and
        // asking the file manager for a folder that is not there shows nothing at all.
        match std::fs::create_dir_all(&dir) {
            Ok(()) => cx.open_with_system(&dir),
            Err(error) => self.set_failed_status(
                self.t(Key::LogFolderMissing)
                    .replace("{error}", &error.to_string()),
            ),
        }
        cx.notify();
    }

    /// The start screen's offer to report the crash the previous launch wrote down.
    pub(crate) fn render_crash_offer(&self, cx: &mut Context<Self>) -> Option<AnyElement> {
        let crash = self.crash_offer.as_ref()?;
        let theme = &self.theme;
        let spacing = theme.atelier.spacing;
        let report = crash.clone();
        Some(
            div()
                .id("startup-crash-offer")
                .debug_selector(|| "startup-crash-offer".into())
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap(spacing.sm)
                .px(spacing.xxl)
                .pb(spacing.md)
                .text_size(theme.atelier.typography.size_sm)
                .text_color(theme.text)
                .child(self.t(Key::CrashOffer))
                .child(
                    div()
                        .text_color(theme.text_muted)
                        .child(bounded(crash.summary(), 200)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap(spacing.sm)
                        .child(
                            atelier::Button::new(
                                "startup-crash-report",
                                self.t(Key::CrashOfferReport),
                            )
                            .primary()
                            .on_click(cx.listener(
                                move |this, _, _, cx| {
                                    this.crash_offer = None;
                                    this.open_crash_report(report.clone(), cx);
                                    cx.notify();
                                },
                            )),
                        )
                        .child(
                            atelier::Button::new(
                                "startup-crash-folder",
                                self.t(Key::CmdOpenLogFolder),
                            )
                            .on_click(cx.listener(|this, _, _, cx| this.open_log_folder(cx))),
                        )
                        .child(
                            atelier::Button::new(
                                "startup-crash-dismiss",
                                self.t(Key::CrashOfferDismiss),
                            )
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.crash_offer = None;
                                cx.notify();
                            })),
                        ),
                )
                .into_any_element(),
        )
    }
}

impl IssueReportWindow {
    fn t(&self, key: Key) -> &'static str {
        key.get(self.language)
    }

    fn invalidate(&mut self) {
        self.diagnosis = None;
        self.ai_confirmation = None;
        self.draft.clear_analysis();
        self.status.clear();
    }

    fn sync_draft(&mut self) {
        self.draft.title = bounded(self.fields[0].content(), 160).replace(['\r', '\n'], " ");
        self.draft.description = bounded(self.fields[1].content(), 12_000);
    }

    fn review_context(&mut self, kind: ContextKind, cx: &mut Context<Self>) {
        self.reviewed = Some(kind);
        if self.loaded[kind.index()] || self.context_rx.is_some() {
            cx.notify();
            return;
        }
        let prompts = self.prompt_snapshot.clone();
        let paths = self.private_paths.clone();
        let (sender, receiver) = mpsc::channel();
        self.context_rx = Some((kind, receiver));
        let spawned = std::thread::Builder::new()
            .name("auris-report-context".into())
            .spawn(move || {
                let result = match kind {
                    ContextKind::MusicPreferences => {
                        auris_session::issue_report::preference_preview()
                    }
                    ContextKind::CompositionPrompts => (|| {
                        let mut result = prompts;
                        let document = auris_session::workshop::Store::library()
                            .active_thread()?
                            .load()?;
                        for message in document.messages.iter().filter(|m| m.user).rev().take(12) {
                            result.push_str("\n\nSong request:\n");
                            result.push_str(&bounded(&message.text, 1500));
                        }
                        Ok(result)
                    })(),
                    ContextKind::UsageLogs | ContextKind::CrashReport => Ok(String::new()),
                }
                .map(|text| redact(&text, &paths));
                let _ = sender.send(result);
            });
        if let Err(error) = spawned {
            self.context_rx = None;
            self.status = error.to_string();
        }
        self.watch(cx);
        cx.notify();
    }

    fn watch(&self, cx: &mut Context<Self>) {
        cx.spawn(async |this, cx| {
            loop {
                let pending = this.update(cx, |this, cx| {
                    let loaded = this
                        .context_rx
                        .as_ref()
                        .and_then(|(_, rx)| match rx.try_recv() {
                            Ok(result) => Some(result),
                            Err(mpsc::TryRecvError::Empty) => None,
                            Err(mpsc::TryRecvError::Disconnected) => {
                                Some(Err("Local context reader stopped".into()))
                            }
                        });
                    if let Some(result) = loaded
                        && let Some((kind, _)) = this.context_rx.take()
                    {
                        match result {
                            Ok(text) => {
                                this.fields[kind.index() + 2] = TextField::new(text);
                                this.loaded[kind.index()] = true;
                            }
                            Err(error) => this.status = error,
                        }
                        cx.notify();
                    }
                    if let Some(result) = this.diagnosis.as_ref().and_then(|job| job.poll()) {
                        this.diagnosis = None;
                        match result {
                            Ok(text) => this.draft.set_analysis(&text),
                            Err(error) => this.status = redact(&error, &this.private_paths),
                        }
                        this.preview = true;
                        cx.notify();
                    }
                    this.context_rx.is_some() || this.diagnosis.is_some()
                });
                if !matches!(pending, Ok(true)) {
                    break;
                }
                cx.background_executor()
                    .timer(std::time::Duration::from_millis(100))
                    .await;
            }
        })
        .detach();
    }

    fn set_consent(&mut self, kind: ContextKind, allow: bool) {
        self.invalidate();
        if allow && self.loaded[kind.index()] {
            self.draft
                .approve_context(kind, self.fields[kind.index() + 2].content().to_string());
        } else {
            self.draft.revoke_context(kind);
        }
    }

    fn start_diagnosis(&mut self, cx: &mut Context<Self>) {
        let Some(body) = self.ai_confirmation.take() else {
            return;
        };
        self.sync_draft();
        if body != self.draft.markdown() {
            return;
        }
        match auris_agent::issue_diagnosis::DiagnosisJob::start(
            self.prefs.clone(),
            body,
            self.language.code(),
        ) {
            Ok(job) => {
                self.diagnosis = Some(job);
                self.status.clear();
                self.watch(cx);
            }
            Err(error) => self.status = error,
        }
        cx.notify();
    }

    fn editor(&self, index: usize, cx: &mut Context<Self>) -> AnyElement {
        let field = &self.fields[index];
        let focus = self.foci[index].clone();
        let pointer_focus = focus.clone();
        let viewport = self.viewports[index].clone();
        let clicks = viewport.clone();
        div()
            .id(("issue-field", index))
            .debug_selector(move || format!("issue-field-{index}"))
            .track_focus(&focus)
            .tab_index(0)
            .w_full()
            .min_w_0()
            .flex_shrink_0()
            .map(|el| if index == 0 { el.h_10() } else { el.h_48() })
            .bg(self.theme.surface_sunken)
            .border_1()
            .rounded(Metrics::RADIUS_SM)
            .border_color(self.theme.border)
            .focus(|el| el.border_color(self.theme.accent))
            .cursor_text()
            .child(editable_scrolling_area(
                field.content().to_string().into(),
                field.selection(),
                field.marked(),
                self.active == Some(index),
                focus,
                cx.entity(),
                self.theme.clone(),
                viewport,
            ))
            .on_mouse_down(
                gpui::MouseButton::Left,
                cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                    if this.fields[index].marked().is_none()
                        && let Some(offset) = clicks.borrow().offset_at(
                            window,
                            this.fields[index].content(),
                            event.position,
                        )
                    {
                        this.fields[index].place_caret(offset, event.modifiers.shift);
                    }
                    this.active = Some(index);
                    window.focus(&pointer_focus);
                    cx.notify();
                }),
            )
            .on_scroll_wheel(
                cx.listener(move |this, event: &gpui::ScrollWheelEvent, _, cx| {
                    this.viewports[index].borrow_mut().scroll_by(
                        event
                            .delta
                            .pixel_delta(crate::ui::text_area::AREA_LINE_HEIGHT)
                            .y,
                    );
                    cx.stop_propagation();
                    cx.notify();
                }),
            )
            .into_any_element()
    }

    fn category_key(kind: ContextKind) -> Key {
        match kind {
            ContextKind::UsageLogs => Key::IssueLogs,
            ContextKind::CompositionPrompts => Key::IssuePrompts,
            ContextKind::MusicPreferences => Key::IssuePreferences,
            ContextKind::CrashReport => Key::IssueCrashReport,
        }
    }

    fn context_row(&self, kind: ContextKind, cx: &mut Context<Self>) -> AnyElement {
        let theme = &self.theme;
        let included = self.draft.includes(kind);
        let mut row =
            div().flex().flex_col().gap_2().min_w_0().child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .child(self.t(Self::category_key(kind))),
                    )
                    .child(div().text_xs().text_color(theme.text_muted).child(self.t(
                        if included {
                            Key::IssueIncluded
                        } else {
                            Key::IssueExcluded
                        },
                    )))
                    .child(button_enabled(
                        ("issue-review", kind.index()),
                        self.t(Key::IssueReviewContext),
                        ButtonStyle::Normal,
                        ButtonState::available(false, self.context_rx.is_none()),
                        theme.accent,
                        theme,
                        cx.listener(move |this, _, _, cx| this.review_context(kind, cx)),
                    )),
            );
        if self.reviewed == Some(kind) {
            if self.loaded[kind.index()] {
                row = row
                    .child(self.editor(kind.index() + 2, cx))
                    .when(self.fields[kind.index() + 2].content().is_empty(), |el| {
                        el.child(self.t(Key::IssueNoContext))
                    })
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                ("issue-allow", kind.index()),
                                self.t(Key::IssueAllowContext),
                                ButtonStyle::Normal,
                                included,
                                theme.accent,
                                theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_consent(kind, true);
                                    cx.notify();
                                }),
                            ))
                            .child(button(
                                ("issue-exclude", kind.index()),
                                self.t(Key::IssueExcludeContext),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                theme,
                                cx.listener(move |this, _, _, cx| {
                                    this.set_consent(kind, false);
                                    cx.notify();
                                }),
                            )),
                    );
            } else if self.context_rx.is_some() {
                row = row.child(self.t(Key::IssueLoading));
            }
        }
        row.into_any_element()
    }

    fn on_key(&mut self, event: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = event.keystroke.key.as_str();
        self.active = self.foci.iter().position(|focus| focus.is_focused(window));
        if self
            .active
            .is_some_and(|index| self.fields[index].marked().is_some())
        {
            return;
        }
        if key == "tab" {
            if event.keystroke.modifiers.shift {
                window.focus_prev();
            } else {
                window.focus_next();
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        if key == "escape" {
            if self.ai_confirmation.take().is_none() {
                window.remove_window();
            }
            cx.stop_propagation();
            cx.notify();
            return;
        }
        self.active = self.foci.iter().position(|focus| focus.is_focused(window));
        if let Some(index) = self.active {
            let modifiers = event.keystroke.modifiers;
            let effect = match key {
                "enter" if index != 0 => {
                    self.fields[index].insert("\n");
                    KeyEffect::Changed
                }
                "up" => {
                    self.fields[index].move_up(modifiers.shift);
                    KeyEffect::Moved
                }
                "down" => {
                    self.fields[index].move_down(modifiers.shift);
                    KeyEffect::Moved
                }
                _ => self.fields[index].apply_key_with_clipboard(
                    key,
                    modifiers.shift,
                    modifiers.secondary(),
                    index != 0,
                    cx,
                ),
            };
            if effect != KeyEffect::Ignored {
                if effect == KeyEffect::Changed {
                    self.text_changed();
                } else {
                    self.viewports[index].borrow_mut().follow_caret();
                }
                cx.stop_propagation();
                cx.notify();
            }
        }
    }
}

impl HasTextField for IssueReportWindow {
    fn field(&mut self) -> Option<&mut TextField> {
        self.active.map(|i| &mut self.fields[i])
    }
    fn readable_field(&self) -> Option<&TextField> {
        self.active.map(|i| &self.fields[i])
    }
    fn text_changed(&mut self) {
        self.invalidate();
        if let Some(index) = self.active {
            self.viewports[index].borrow_mut().follow_caret();
            if index >= 2 {
                self.draft.revoke_context(ContextKind::ALL[index - 2]);
            }
        }
        self.sync_draft();
    }
}
crate::entity_input_handler!(IssueReportWindow);

impl Render for IssueReportWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.theme = cx.global::<Theme>().clone();
        if !self.focus.contains_focused(window, cx) {
            window.focus(&self.foci[0]);
        }
        self.active = self.foci.iter().position(|f| f.is_focused(window));
        self.sync_draft();
        let theme = self.theme.clone();
        let titlebar = titlebar::bar(
            window,
            &theme,
            titlebar::Parts::new("issue-title", self.t(Key::IssueDraft)),
            titlebar::controls(window, &theme, |_, window, _| window.remove_window()),
        );
        let ai_available = auris_agent::input_suggestion::supported(&self.prefs);
        let ready =
            !self.draft.title.trim().is_empty() && !self.draft.description.trim().is_empty();
        let body = self.draft.markdown();
        let mut content = div()
            .flex()
            .flex_col()
            .gap_4()
            .min_w_0()
            .children(self.proposal.as_ref().map(|proposal| {
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.t(Key::IssueProposalPrivacy))
                    .child(proposal.title.clone())
                    .child(proposal.explanation.clone())
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                "issue-proposal-allow",
                                self.t(Key::IssueProposalAllow),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    if let Some(proposal) = this.proposal.take() {
                                        this.invalidate();
                                        this.fields[0] = TextField::new(proposal.title);
                                        this.fields[1] = TextField::new(proposal.explanation);
                                        this.sync_draft();
                                        cx.notify();
                                    }
                                }),
                            ))
                            .child(button(
                                "issue-proposal-exclude",
                                self.t(Key::IssueExcludeContext),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.proposal = None;
                                    cx.notify();
                                }),
                            )),
                    )
            }))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.t(Key::IssueTitle))
                    .child(self.editor(0, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.t(Key::IssueDescription))
                    .child(self.editor(1, cx)),
            )
            .child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::IssuePrivacy)),
            )
            .children(
                ContextKind::ALL
                    .into_iter()
                    .filter(|kind| *kind != ContextKind::CrashReport || self.has_crash)
                    .map(|kind| self.context_row(kind, cx)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "issue-preview",
                        self.t(Key::IssuePreview),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.preview = !this.preview;
                            cx.notify();
                        }),
                    ))
                    .child(button_enabled(
                        "issue-investigate",
                        self.t(Key::IssueInvestigate),
                        ButtonStyle::Normal,
                        ButtonState::available(
                            false,
                            ai_available && ready && self.diagnosis.is_none(),
                        ),
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.draft.clear_analysis();
                            this.sync_draft();
                            this.ai_confirmation = Some(this.draft.markdown());
                            this.preview = true;
                            cx.notify();
                        }),
                    )),
            );
        if !ai_available {
            content = content.child(
                div()
                    .text_xs()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::IssueAiUnavailable)),
            );
        }
        if self.diagnosis.is_some() {
            content = content.child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(self.t(Key::IssueAiWorking))
                    .child(button(
                        "issue-stop",
                        self.t(Key::Cancel),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        &theme,
                        cx.listener(|this, _, _, cx| {
                            this.invalidate();
                            this.status = this.t(Key::IssueAiStopped).into();
                            cx.notify();
                        }),
                    )),
            );
        }
        if self.ai_confirmation.is_some() {
            content = content.child(
                div()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(self.t(Key::IssueAiConsent))
                    .child(div().text_xs().text_color(theme.text_muted).child(format!(
                        "{} / {} / {}",
                        self.prefs.backend, self.prefs.model, self.prefs.url
                    )))
                    .child(
                        div()
                            .flex()
                            .flex_wrap()
                            .gap_2()
                            .child(button(
                                "issue-ai-send",
                                self.t(Key::IssueAiSend),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| this.start_diagnosis(cx)),
                            ))
                            .child(button(
                                "issue-ai-cancel",
                                self.t(Key::Cancel),
                                ButtonStyle::Normal,
                                false,
                                theme.accent,
                                &theme,
                                cx.listener(|this, _, _, cx| {
                                    this.ai_confirmation = None;
                                    cx.notify();
                                }),
                            )),
                    ),
            );
        }
        if self.preview {
            content = content
                .child(
                    div()
                        .id("issue-body-preview")
                        .debug_selector(|| "issue-body-preview".into())
                        .w_full()
                        .min_w_0()
                        .p_3()
                        .rounded(Metrics::RADIUS_SM)
                        .bg(theme.surface_sunken)
                        .text_xs()
                        .child(body.clone()),
                )
                .child(button(
                    "issue-remove-analysis",
                    self.t(Key::IssueAiRemove),
                    ButtonStyle::Ghost,
                    false,
                    theme.accent,
                    &theme,
                    cx.listener(|this, _, _, cx| {
                        this.invalidate();
                        cx.notify();
                    }),
                ))
                .child(
                    div()
                        .text_xs()
                        .text_color(theme.text_muted)
                        .child(self.t(Key::IssueGithubHelp)),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_2()
                        .child(button_enabled(
                            "issue-copy",
                            self.t(Key::IssueCopy),
                            ButtonStyle::Normal,
                            ButtonState::available(false, ready),
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.sync_draft();
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    this.draft.markdown(),
                                ));
                                this.status = this.t(Key::IssueCopied).into();
                                cx.notify();
                            }),
                        ))
                        .child(button_enabled(
                            "issue-github",
                            self.t(Key::IssueGithub),
                            ButtonStyle::Normal,
                            ButtonState::available(false, ready),
                            theme.accent,
                            &theme,
                            cx.listener(|this, _, _, cx| {
                                this.sync_draft();
                                cx.write_to_clipboard(gpui::ClipboardItem::new_string(
                                    this.draft.markdown(),
                                ));
                                cx.open_url("https://github.com/uthree/auris-studio/issues/new");
                            }),
                        )),
                );
        }
        div()
            .id("issue-root")
            .track_focus(&self.focus)
            .flex()
            .flex_col()
            .size_full()
            .bg(theme.atelier.window_fill())
            .text_color(theme.text)
            .font(theme.font.clone())
            .text_sm()
            .on_key_down(cx.listener(Self::on_key))
            .child(titlebar)
            .child(
                div()
                    .id("issue-scroll")
                    .flex_1()
                    .min_h_0()
                    .min_w_0()
                    .overflow_y_scroll()
                    .track_scroll(&self.body_scroll)
                    .p_4()
                    .child(content),
            )
            .when(!self.status.is_empty(), |el| {
                el.child(div().p_2().text_xs().child(self.status.clone()))
            })
    }
}

/// `paths`, followed by every other spelling the system gives the same folders.
///
/// A Windows user name longer than eight characters, or one with a space in it, gives the
/// profile an 8.3 short name as well, and the temporary folder is usually reached through it
/// (`C:\Users\ALEXAN~1\AppData\Local\Temp`). Text that names it that way would pass redaction
/// untouched, so the short spelling is redacted too.
fn with_short_spellings(mut paths: Vec<String>) -> Vec<String> {
    for index in 0..paths.len() {
        if let Some(short) = short_spelling(&paths[index])
            && !paths.contains(&short)
        {
            paths.push(short);
        }
    }
    paths
}

/// The 8.3 short spelling of `path`, when the volume keeps one and it differs.
#[cfg(target_os = "windows")]
fn short_spelling(path: &str) -> Option<String> {
    use windows::Win32::Storage::FileSystem::GetShortPathNameW;
    use windows::core::HSTRING;

    let long = HSTRING::from(path);
    // SAFETY: `long` is a NUL-terminated wide string that outlives both calls, and the second
    // call is given a buffer of the length the first one asked for.
    let needed = unsafe { GetShortPathNameW(&long, None) } as usize;
    if needed == 0 {
        return None;
    }
    let mut buffer = vec![0_u16; needed];
    let written = unsafe { GetShortPathNameW(&long, Some(&mut buffer)) } as usize;
    if written == 0 || written >= buffer.len() {
        return None;
    }
    let short = String::from_utf16(&buffer[..written]).ok()?;
    (short != path).then_some(short)
}

#[cfg(not(target_os = "windows"))]
fn short_spelling(_path: &str) -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{TestAppContext, VisualTestContext, WindowHandle, point};

    #[test]
    fn every_added_spelling_names_the_same_folder() {
        let root = tempfile::tempdir().unwrap();
        let folder = root.path().join("A folder name long enough");
        std::fs::create_dir(&folder).unwrap();
        let long = folder.display().to_string();
        let spellings = with_short_spellings(vec![long.clone(), long.clone()]);
        assert_eq!(&spellings[..2], [long.clone(), long.clone()]);
        // Whether a short name exists depends on the volume, so only what was added is checked.
        for added in &spellings[2..] {
            assert_ne!(added, &long);
            assert_eq!(
                std::fs::canonicalize(added).unwrap(),
                std::fs::canonicalize(&folder).unwrap()
            );
        }
        assert!(with_short_spellings(vec!["no such folder anywhere".into()]).len() == 1);
    }

    #[gpui::test]
    fn a_crash_report_is_shown_for_review_and_included_only_when_allowed(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        let crash = auris_session::diagnostics::CrashReport {
            path: "crash-1.txt".into(),
            name: "crash-1.txt".into(),
            text:
                "Auris Studio crash report\n\nPanic: index out of bounds\nBacktrace:\n   0: main\n"
                    .into(),
        };
        app.update(cx, |this, cx| {
            this.crash_offer = Some(crash.clone());
            assert!(this.render_crash_offer(cx).is_some());
            this.open_crash_report(crash, cx);
        });
        let handle = app.read_with(cx, |this, _| this.issue_report_window.unwrap());
        handle
            .update(cx, |this, _, _| {
                assert!(this.has_crash);
                assert_eq!(this.reviewed, Some(ContextKind::CrashReport));
                assert_eq!(this.fields[0].content(), "Crash: index out of bounds");
                let field = &this.fields[2 + ContextKind::CrashReport.index()];
                assert!(field.content().contains("Backtrace:"));
                // Shown is not included: the report quotes paths and log lines.
                this.sync_draft();
                assert!(!this.draft.includes(ContextKind::CrashReport));
                assert!(!this.draft.markdown().contains("## Crash report"));
                this.set_consent(ContextKind::CrashReport, true);
                let body = this.draft.markdown();
                assert!(body.contains("## Crash report"), "{body}");
                assert!(body.contains("    Panic: index out of bounds"), "{body}");
            })
            .unwrap();
    }

    #[gpui::test]
    fn a_report_without_a_crash_has_no_crash_row(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_issue_report(None, cx));
        let handle = app.read_with(cx, |this, _| this.issue_report_window.unwrap());
        handle
            .read_with(cx, |this, _| {
                assert!(!this.has_crash);
                assert_eq!(this.reviewed, None);
                assert!(this.fields[0].content().is_empty());
            })
            .unwrap();
    }

    #[gpui::test]
    fn report_uses_configured_agent_before_the_chat_panel_is_opened(cx: &mut TestAppContext) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.settings.agent.backend = "rig".into();
            this.settings.agent.model = "report-fixture".into();
            assert!(this.settings.agent.is_configured());
            assert!(!this.agent_chat.preferences().is_configured());
            this.open_issue_report(None, cx);
        });
        let handle = app.read_with(cx, |this, _| this.issue_report_window.unwrap());
        handle
            .read_with(cx, |this, _| {
                assert_eq!(this.prefs.model, "report-fixture");
                assert!(auris_agent::input_suggestion::supported(&this.prefs));
            })
            .unwrap();
    }

    fn reveal_and_click(
        selector: &'static str,
        handle: WindowHandle<IssueReportWindow>,
        cx: &mut VisualTestContext,
    ) {
        cx.run_until_parked();
        let bounds = cx
            .debug_bounds(selector)
            .unwrap_or_else(|| panic!("missing {selector}"));
        handle
            .update(cx, |this, _, cx| {
                let previous = this.body_scroll.offset();
                this.body_scroll
                    .set_offset(point(px(0.0), previous.y + px(260.0) - bounds.center().y));
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click(selector, cx);
        cx.run_until_parked();
    }

    #[gpui::test]
    fn issue_context_requires_explicit_consent_and_edits_revoke_derived_output(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| this.open_issue_report(None, cx));
        let handle = app.read_with(cx, |this, _| this.issue_report_window.unwrap());
        let cx = &mut VisualTestContext::from_window(handle.into(), cx);
        handle
            .update(cx, |this, _, cx| {
                this.fields[0] = TextField::new("Reproduction");
                this.fields[2] = TextField::new("PRIVATE_LOG");
                this.fields[3] = TextField::new("PRIVATE_PROMPT");
                this.fields[4] = TextField::new("PRIVATE_TASTE");
                this.loaded = [true; ContextKind::ALL.len()];
                this.reviewed = Some(ContextKind::UsageLogs);
                this.sync_draft();
                assert!(!this.draft.markdown().contains("PRIVATE"));
                cx.notify();
            })
            .unwrap();
        reveal_and_click("issue-allow-0", handle, cx);
        handle
            .update(cx, |this, _, _| {
                let body = this.draft.markdown();
                assert!(body.contains("PRIVATE_LOG"));
                assert!(!body.contains("PRIVATE_PROMPT"));
                assert!(!body.contains("PRIVATE_TASTE"));
                this.draft.set_analysis("DERIVED_PRIVATE_LOG");
            })
            .unwrap();
        reveal_and_click("issue-exclude-0", handle, cx);
        handle
            .read_with(cx, |this, _| {
                assert!(!this.draft.markdown().contains("PRIVATE"))
            })
            .unwrap();
        reveal_and_click("issue-allow-0", handle, cx);
        reveal_and_click("issue-field-2", handle, cx);
        cx.simulate_keystrokes("left");
        handle
            .read_with(cx, |this, _| {
                assert!(this.draft.includes(ContextKind::UsageLogs))
            })
            .unwrap();
        cx.simulate_input("edited");
        handle
            .read_with(cx, |this, _| {
                assert!(!this.draft.includes(ContextKind::UsageLogs))
            })
            .unwrap();
        reveal_and_click("issue-preview", handle, cx);
        reveal_and_click("issue-copy", handle, cx);
        assert!(
            !cx.read_from_clipboard()
                .unwrap()
                .text()
                .unwrap()
                .contains("PRIVATE")
        );
    }

    #[gpui::test]
    fn issue_ai_confirmation_is_separate_and_closing_resets_all_context_consent(
        cx: &mut TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |this, cx| {
            this.open_issue_report(
                Some(Suggestion {
                    title: "Missing feature".into(),
                    explanation: "Confirmed limitation".into(),
                }),
                cx,
            )
        });
        let handle = app.read_with(cx, |this, _| this.issue_report_window.unwrap());
        let report_cx = &mut VisualTestContext::from_window(handle.into(), cx);
        handle
            .read_with(report_cx, |this, _| {
                assert!(!this.draft.markdown().contains("Confirmed limitation"));
            })
            .unwrap();
        reveal_and_click("issue-proposal-allow", handle, report_cx);
        handle
            .read_with(report_cx, |this, _| {
                assert!(this.draft.markdown().contains("Confirmed limitation"));
            })
            .unwrap();
        handle
            .update(report_cx, |this, _, cx| {
                this.prefs.backend = "rig".into();
                this.prefs.model = "fixture".into();
                this.fields[2] = TextField::new("APPROVED_LOG");
                this.fields[3] = TextField::new("UNAPPROVED_PROMPT");
                this.set_consent(ContextKind::UsageLogs, true);
                cx.notify();
            })
            .unwrap();
        reveal_and_click("issue-investigate", handle, report_cx);
        handle
            .update(report_cx, |this, _, _| {
                let body = this.ai_confirmation.as_ref().unwrap();
                assert!(body.contains("APPROVED_LOG"));
                assert!(!body.contains("UNAPPROVED_PROMPT"));
                assert!(
                    this.diagnosis.is_none(),
                    "opening confirmation must not contact a provider"
                );
                this.set_consent(ContextKind::UsageLogs, false);
                assert!(this.ai_confirmation.is_none());
            })
            .unwrap();
        handle
            .update(report_cx, |_, window, _| window.remove_window())
            .unwrap();
        report_cx.run_until_parked();
        app.update(report_cx, |this, cx| this.open_issue_report(None, cx));
        let next = app.read_with(report_cx, |this, _| this.issue_report_window.unwrap());
        next.read_with(report_cx, |this, _| {
            assert!(
                ContextKind::ALL
                    .into_iter()
                    .all(|kind| !this.draft.includes(kind))
            );
            assert!(this.ai_confirmation.is_none());
        })
        .unwrap();
    }
}
