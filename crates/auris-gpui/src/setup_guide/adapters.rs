//! User-initiated installation of the optional ACP adapters.

use std::sync::mpsc::{self, Receiver, TryRecvError};

use auris_agent::{
    AcpAdapter, AdapterInstall, AdapterInstallEvent, AdapterSetupProblem, AdapterSetupStatus,
};
use gpui::{Context, IntoElement, div, prelude::*};

use super::SetupGuide;
use crate::ui::widgets::{ButtonState, ButtonStyle, Latch, button, button_enabled};
use auris_i18n::{Key, Language};

#[derive(Default)]
struct AdapterRow {
    status: Option<AdapterSetupStatus>,
    phase: Option<AdapterInstallEvent>,
    job: Option<AdapterInstall>,
    cancelling: bool,
    #[cfg(test)]
    test_events: Option<Receiver<AdapterInstallEvent>>,
}

impl AdapterRow {
    fn busy(&self) -> bool {
        matches!(
            self.phase,
            Some(
                AdapterInstallEvent::Preparing
                    | AdapterInstallEvent::Installing
                    | AdapterInstallEvent::Verifying
            )
        )
    }

    fn can_install(&self) -> bool {
        !self.busy()
            && self
                .status
                .as_ref()
                .is_some_and(|status| !status.installed && status.problem.is_none())
    }

    fn start(&mut self, adapter: AcpAdapter) {
        if !self.can_install() {
            return;
        }
        self.phase = Some(AdapterInstallEvent::Preparing);
        self.cancelling = false;
        // Harness gestures must never install packages on the developer's machine.
        if !cfg!(test) {
            self.job = Some(auris_agent::install_acp_adapter_background(adapter));
        }
    }

    fn cancel(&mut self) {
        if self.busy() {
            self.cancelling = true;
            if let Some(job) = &self.job {
                job.cancel();
            }
        }
    }

    fn poll(&mut self) {
        if !self.busy() {
            return;
        }
        loop {
            let receiver = self.job.as_ref().map(|job| &job.events);
            #[cfg(test)]
            let receiver = receiver.or(self.test_events.as_ref());
            let Some(receiver) = receiver else { break };
            match receiver.try_recv() {
                Ok(event) => {
                    let complete = matches!(event, AdapterInstallEvent::Complete);
                    let terminal = matches!(
                        event,
                        AdapterInstallEvent::Complete
                            | AdapterInstallEvent::Cancelled
                            | AdapterInstallEvent::Failed(_)
                    );
                    self.phase = Some(event);
                    if complete {
                        self.status = Some(AdapterSetupStatus {
                            installed: true,
                            problem: None,
                        });
                    }
                    if terminal {
                        self.job = None;
                        self.cancelling = false;
                        break;
                    }
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    self.phase = Some(AdapterInstallEvent::Failed(String::new()));
                    self.job = None;
                    self.cancelling = false;
                    break;
                }
            }
        }
    }

    fn label(&self, language: Language) -> String {
        let key = if self.cancelling {
            Key::SetupAdapterCancelling
        } else if let Some(phase) = &self.phase {
            match phase {
                AdapterInstallEvent::Preparing => Key::SetupAdapterPreparing,
                AdapterInstallEvent::Installing => Key::SetupAdapterInstalling,
                AdapterInstallEvent::Verifying => Key::SetupAdapterVerifying,
                AdapterInstallEvent::Complete => Key::SetupAdapterInstalled,
                AdapterInstallEvent::Cancelled => Key::SetupAdapterCancelled,
                AdapterInstallEvent::Failed(_) => Key::SetupAdapterFailed,
            }
        } else if let Some(status) = &self.status {
            match status.problem {
                Some(AdapterSetupProblem::NodeMissing) => Key::SetupAdapterNodeMissing,
                Some(AdapterSetupProblem::NodeTooOld) => Key::SetupAdapterNodeOld,
                Some(AdapterSetupProblem::NpmMissing) => Key::SetupAdapterNpmMissing,
                Some(AdapterSetupProblem::Other(_)) => Key::SetupAdapterCheckFailed,
                None if status.installed => Key::SetupAdapterInstalled,
                None => Key::SetupAdapterMissing,
            }
        } else {
            Key::AgentProviderChecking
        };
        key.get(language).to_owned()
    }

    fn detail(&self) -> Option<&str> {
        if let Some(AdapterInstallEvent::Failed(error)) = &self.phase {
            return (!error.is_empty()).then_some(error.as_str());
        }
        match self
            .status
            .as_ref()
            .and_then(|status| status.problem.as_ref())
        {
            Some(AdapterSetupProblem::Other(error)) => Some(error),
            _ => None,
        }
    }
}

/// Setup-only state. The guide owns jobs, so closing it also cancels them.
pub(super) struct AdapterSetup {
    rows: [AdapterRow; 2],
    checks: Option<Receiver<(AcpAdapter, AdapterSetupStatus)>>,
}

fn index(adapter: AcpAdapter) -> usize {
    match adapter {
        AcpAdapter::Codex => 0,
        AcpAdapter::Claude => 1,
    }
}

impl AdapterSetup {
    pub(super) fn new() -> Self {
        let mut setup = Self {
            rows: Default::default(),
            checks: None,
        };
        setup.check();
        setup
    }

    fn check(&mut self) {
        if self.checks.is_some() || self.rows.iter().any(AdapterRow::busy) {
            return;
        }
        for row in &mut self.rows {
            row.status = None;
            row.phase = None;
        }
        self.checks = Some(if cfg!(test) {
            // Deterministic offline prerequisites for the window harness.
            let (sender, receiver) = mpsc::channel();
            for adapter in AcpAdapter::ALL {
                let _ = sender.send((
                    adapter,
                    AdapterSetupStatus {
                        installed: false,
                        problem: None,
                    },
                ));
            }
            receiver
        } else {
            auris_agent::inspect_acp_adapters_background()
        });
    }

    pub(super) fn poll(&mut self) {
        if let Some(checks) = &self.checks {
            loop {
                match checks.try_recv() {
                    Ok((adapter, status)) => self.rows[index(adapter)].status = Some(status),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        for row in &mut self.rows {
                            if row.status.is_none() {
                                row.status = Some(AdapterSetupStatus {
                                    installed: false,
                                    problem: Some(AdapterSetupProblem::Other(String::new())),
                                });
                            }
                        }
                        self.checks = None;
                        break;
                    }
                }
            }
        }
        for row in &mut self.rows {
            row.poll();
        }
    }

    pub(super) fn cancel_all(&mut self) {
        for row in &mut self.rows {
            row.cancel();
        }
    }
}

impl SetupGuide {
    pub(super) fn adapter_controls(&self, cx: &mut Context<Self>) -> impl IntoElement + use<> {
        let theme = &self.theme;
        let mut section = div()
            .flex()
            .flex_col()
            .flex_shrink_0()
            .min_w_0()
            .gap_3()
            .child(div().child(self.t(Key::SetupAdapters)))
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SetupAdaptersHelp)),
            );
        for adapter in AcpAdapter::ALL {
            let i = index(adapter);
            let row = &self.adapters.rows[i];
            let mut heading = div()
                .flex()
                .flex_wrap()
                .items_center()
                .justify_between()
                .gap_2()
                .child(div().child(format!("{} · {}", adapter.name(), adapter.version())));
            let busy = row.busy();
            let enabled = if busy {
                !row.cancelling
            } else {
                row.can_install()
            };
            let label = if busy {
                Key::Cancel
            } else if matches!(
                row.phase,
                Some(AdapterInstallEvent::Failed(_) | AdapterInstallEvent::Cancelled)
            ) {
                Key::SetupAdapterRetry
            } else {
                Key::SetupAdapterInstall
            };
            if !row.status.as_ref().is_some_and(|status| status.installed) || busy {
                heading = heading.child(button_enabled(
                    ("setup-adapter-install", i),
                    self.t(label),
                    ButtonStyle::Normal,
                    if enabled {
                        ButtonState::Enabled(Latch::Off)
                    } else {
                        ButtonState::Disabled
                    },
                    theme.accent,
                    theme,
                    cx.listener(move |this, _, _, cx| {
                        let row = &mut this.adapters.rows[i];
                        if row.busy() {
                            row.cancel();
                        } else {
                            row.start(adapter);
                        }
                        cx.notify();
                    }),
                ));
            }
            let mut card = div()
                .flex()
                .flex_col()
                .flex_shrink_0()
                .gap_2()
                .py_3()
                .border_t_1()
                .border_color(theme.border)
                .child(heading)
                .child(
                    div()
                        .id(("setup-adapter-status", i))
                        .when(
                            row.status.as_ref().is_some_and(|status| status.installed),
                            |this| this.debug_selector(move || format!("setup-adapter-ready-{i}")),
                        )
                        .text_color(theme.text_muted)
                        .child(row.label(self.language)),
                );
            if let Some(detail) = row.detail() {
                card = card.child(div().text_color(theme.danger).child(detail.to_owned()));
            }
            section = section.child(card);
        }
        let can_check =
            self.adapters.checks.is_none() && !self.adapters.rows.iter().any(AdapterRow::busy);
        section
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "setup-adapter-node",
                        self.t(Key::SetupAdapterGetNode),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        theme,
                        |_, _, cx| cx.open_url("https://nodejs.org/en/download"),
                    ))
                    .child(button_enabled(
                        "setup-adapter-check",
                        self.t(Key::SetupAdapterCheck),
                        ButtonStyle::Normal,
                        if can_check {
                            ButtonState::Enabled(Latch::Off)
                        } else {
                            ButtonState::Disabled
                        },
                        theme.accent,
                        theme,
                        cx.listener(|this, _, _, cx| {
                            this.adapters.check();
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .text_color(theme.text_muted)
                    .child(self.t(Key::SetupAdaptersLogin)),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(button(
                        "setup-codex-help",
                        self.t(Key::SetupCodexHelp),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        theme,
                        |_, _, cx| cx.open_url("https://developers.openai.com/codex/cli"),
                    ))
                    .child(button(
                        "setup-claude-help",
                        self.t(Key::SetupClaudeHelp),
                        ButtonStyle::Normal,
                        false,
                        theme.accent,
                        theme,
                        |_, _, cx| cx.open_url("https://code.claude.com/docs/en/setup"),
                    )),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::{px, size};

    #[gpui::test]
    fn install_gestures_handle_cancel_retry_and_completion_without_editing_project(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        let before = app.read_with(cx, |app, _| app.session.project().clone());
        app.update(cx, |app, cx| app.open_setup_guide(cx));
        let handle = app.read_with(cx, |app, _| app.setup_guide_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(size(px(900.), px(1400.)));
        let (sender, receiver) = mpsc::channel();
        handle
            .update(cx, |guide, _, cx| {
                guide.step = 1;
                guide.adapters.poll();
                guide.adapters.rows[0].test_events = Some(receiver);
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click("setup-adapter-install-0", cx);
        handle
            .update(cx, |guide, _, _| {
                assert!(guide.adapters.rows[0].busy());
                assert!(!guide.adapters.rows[0].can_install());
            })
            .unwrap();
        sender.send(AdapterInstallEvent::Installing).unwrap();
        handle.update(cx, |guide, _, cx| guide.refresh(cx)).unwrap();
        cx.run_until_parked();
        // The button keeps its identity when Install becomes Cancel; Space activates it.
        cx.simulate_event(gpui::KeyUpEvent {
            keystroke: gpui::Keystroke::parse("space").unwrap(),
        });
        handle
            .update(cx, |guide, _, _| assert!(guide.adapters.rows[0].cancelling))
            .unwrap();
        sender.send(AdapterInstallEvent::Cancelled).unwrap();
        handle.update(cx, |guide, _, cx| guide.refresh(cx)).unwrap();
        cx.run_until_parked();
        crate::harness::click("setup-adapter-install-0", cx);
        sender
            .send(AdapterInstallEvent::Failed("Network unavailable".into()))
            .unwrap();
        handle
            .update(cx, |guide, _, cx| {
                guide.refresh(cx);
                assert_eq!(guide.adapters.rows[0].detail(), Some("Network unavailable"));
                assert!(guide.adapters.rows[0].can_install());
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click("setup-adapter-install-0", cx);
        sender.send(AdapterInstallEvent::Verifying).unwrap();
        sender.send(AdapterInstallEvent::Complete).unwrap();
        handle
            .update(cx, |guide, _, cx| {
                guide.refresh(cx);
                assert!(guide.adapters.rows[0].status.as_ref().unwrap().installed);
                assert!(!guide.adapters.rows[0].busy());
            })
            .unwrap();
        cx.run_until_parked();
        // GPUI retains debug bounds for removed elements across reused frames.
        assert!(cx.debug_bounds("setup-adapter-ready-0").is_some());
        app.read_with(cx, |app, _| assert_eq!(app.session.project(), &before));
    }

    #[gpui::test]
    fn missing_node_disables_install_and_footer_remains_visible_in_small_localized_window(
        cx: &mut gpui::TestAppContext,
    ) {
        let (app, cx) = crate::harness::open(cx);
        app.update(cx, |app, cx| app.open_setup_guide(cx));
        let handle = app.read_with(cx, |app, _| app.setup_guide_window.unwrap());
        let cx = &mut gpui::VisualTestContext::from_window(handle.into(), cx);
        cx.simulate_resize(size(px(900.), px(1400.)));
        handle
            .update(cx, |guide, _, cx| {
                guide.step = 1;
                guide.adapters.poll();
                guide.adapters.rows[0].status = Some(AdapterSetupStatus {
                    installed: false,
                    problem: Some(AdapterSetupProblem::NodeMissing),
                });
                cx.notify();
            })
            .unwrap();
        cx.run_until_parked();
        crate::harness::click("setup-adapter-install-0", cx);
        handle
            .update(cx, |guide, _, _| {
                assert!(guide.adapters.rows[0].phase.is_none())
            })
            .unwrap();
        crate::harness::click("setup-adapter-check", cx);
        handle
            .update(cx, |guide, _, cx| {
                guide.refresh(cx);
                assert!(guide.adapters.rows[0].can_install());
            })
            .unwrap();
        cx.simulate_resize(size(px(560.), px(420.)));
        for language in [Language::English, Language::Japanese] {
            handle
                .update(cx, |guide, _, cx| {
                    guide.language = language;
                    cx.notify();
                })
                .unwrap();
            cx.run_until_parked();
            for id in ["setup-later", "setup-back", "setup-next"] {
                let bounds = cx.debug_bounds(id).unwrap();
                assert!(bounds.left() >= px(0.) && bounds.right() <= px(560.));
                assert!(bounds.top() >= px(0.) && bounds.bottom() <= px(420.));
            }
        }
    }

    #[test]
    fn disconnected_installer_releases_busy_state_for_retry() {
        let mut row = AdapterRow {
            status: Some(AdapterSetupStatus {
                installed: false,
                problem: None,
            }),
            ..Default::default()
        };
        row.start(AcpAdapter::Codex);
        row.test_events = Some(mpsc::channel().1);
        row.poll();
        assert!(matches!(row.phase, Some(AdapterInstallEvent::Failed(_))));
        assert!(row.can_install());
    }
}
