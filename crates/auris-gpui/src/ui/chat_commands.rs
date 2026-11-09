//! Local slash commands and completion shared by both conversation composers.
use super::agent_chat::{ModelOption, model_search};
use crate::{app::AurisApp, theme::Theme};
use auris_agent::PhaseCommand;
use auris_i18n::{Key, Language};
use auris_session::agent_policy::OPERATIONS;
use gpui::{AnyElement, IntoElement, MouseButton, MouseDownEvent, div, prelude::*};

/// The conversation whose local commands are being offered.
#[derive(Clone, Copy)]
pub(crate) enum Scope {
    Agent,
    Workshop,
}

impl Scope {
    fn supports(self, command: &str) -> bool {
        matches!(self, Self::Agent) || matches!(command, "/model" | "/effort" | "/help")
    }
}

/// One completion keeps the model's provider route in its replacement text.
pub(crate) struct SlashMatch {
    pub(crate) fill: String,
    pub(crate) label: String,
    pub(crate) description: Key,
}

/// Selection and dismissal belong to each composer, independently of its conversation.
#[derive(Default)]
pub(crate) struct Completions {
    pub(crate) selected: usize,
    pub(crate) dismissed: Option<String>,
    pub(crate) scroll: gpui::ScrollHandle,
}

/// What a candidate key does before the composer handles normal text input.
pub(crate) enum CompletionKey {
    Ignored,
    Handled,
    Complete { fill: String, submit: bool },
}

impl Completions {
    pub(crate) fn reset(&mut self) {
        self.selected = 0;
        self.dismissed = None;
        self.scroll.scroll_to_item(0);
    }

    pub(crate) fn matches(
        &self,
        value: &str,
        models: &[ModelOption],
        scope: Scope,
    ) -> Vec<SlashMatch> {
        if self.dismissed.as_deref() == Some(value) {
            Vec::new()
        } else {
            slash_matches(value, models, scope)
        }
    }

    pub(crate) fn key(
        &mut self,
        value: &str,
        matches: &[SlashMatch],
        event: &gpui::KeyDownEvent,
    ) -> CompletionKey {
        if matches.is_empty() {
            return CompletionKey::Ignored;
        }
        let selected = self.selected % matches.len();
        match event.keystroke.key.as_str() {
            "escape" => self.dismissed = Some(value.to_owned()),
            "up" => {
                self.selected = (selected + matches.len() - 1) % matches.len();
                self.scroll.scroll_to_item(self.selected);
            }
            "down" => {
                self.selected = (selected + 1) % matches.len();
                self.scroll.scroll_to_item(self.selected);
            }
            "tab" => {
                return CompletionKey::Complete {
                    fill: matches[selected].fill.clone(),
                    submit: false,
                };
            }
            "enter" if !event.keystroke.modifiers.shift && matches[selected].fill != value => {
                return CompletionKey::Complete {
                    fill: matches[selected].fill.clone(),
                    submit: !matches[selected].fill.ends_with(' '),
                };
            }
            _ => return CompletionKey::Ignored,
        }
        CompletionKey::Handled
    }

    pub(crate) fn render(
        &self,
        matches: Vec<SlashMatch>,
        scope: Scope,
        theme: &Theme,
        language: Language,
        cx: &mut gpui::Context<AurisApp>,
    ) -> Option<AnyElement> {
        if matches.is_empty() {
            return None;
        }
        let selected = self.selected % matches.len();
        let prefix = match scope {
            Scope::Agent => "agent",
            Scope::Workshop => "workshop",
        };
        Some(
            div()
                .id(gpui::SharedString::from(format!(
                    "{prefix}-slash-completions"
                )))
                .debug_selector(move || format!("{prefix}-slash-completions"))
                .track_scroll(&self.scroll)
                .flex()
                .flex_col()
                .max_h(gpui::px(180.0))
                .overflow_y_scroll()
                .border_t_1()
                .border_color(theme.border)
                .bg(theme.surface_raised)
                .children(matches.into_iter().enumerate().map(|(index, candidate)| {
                    div()
                        .id((
                            gpui::SharedString::from(format!("{prefix}-slash-option")),
                            index,
                        ))
                        .debug_selector(move || format!("{prefix}-slash-option-{index}"))
                        .flex()
                        .items_center()
                        .gap(theme.atelier.spacing.sm)
                        .px(theme.atelier.spacing.sm)
                        .py(theme.atelier.spacing.xs)
                        .when(index == selected, |this| this.bg(theme.surface_hover))
                        .hover(|this| this.bg(theme.surface_hover))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _: &MouseDownEvent, window, cx| {
                                match scope {
                                    Scope::Agent => this.accept_agent_completion(&candidate.fill),
                                    Scope::Workshop => {
                                        this.accept_workshop_completion(&candidate.fill);
                                        this.focus_workshop_input(window);
                                    }
                                }
                                cx.stop_propagation();
                                cx.notify();
                            }),
                        )
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .overflow_hidden()
                                .text_xs()
                                .text_color(theme.text)
                                .child(candidate.label),
                        )
                        .child(
                            div()
                                .flex_shrink_0()
                                .text_xs()
                                .text_color(theme.text_faint)
                                .child(candidate.description.get(language)),
                        )
                }))
                .into_any_element(),
        )
    }
}

/// Localized help always reflects the same catalogue as completion.
pub(crate) fn help(scope: Scope, language: Language) -> String {
    catalogue(scope)
        .map(|(name, _, description)| format!("{name} — {}", description.get(language)))
        .collect::<Vec<_>>()
        .join("\n")
}

/// What a composition command says it does, beside its name.
fn phase_key(command: PhaseCommand) -> Key {
    match command {
        PhaseCommand::Compose => Key::AgentSlashCompose,
        PhaseCommand::Draft => Key::AgentSlashDraft,
        PhaseCommand::Mix => Key::AgentSlashMix,
        PhaseCommand::Review => Key::AgentSlashReview,
    }
}

/// Every command a scope offers: the local controls, then the composition commands, which go
/// to the model as an assignment and so exist only where a model is driven from this panel.
fn catalogue(scope: Scope) -> impl Iterator<Item = (&'static str, bool, Key)> {
    let phases = matches!(scope, Scope::Agent)
        .then_some(PhaseCommand::ALL)
        .into_iter()
        .flatten()
        .map(|command| {
            (
                command.name(),
                command.needs_instruction(),
                phase_key(command),
            )
        });
    SLASH_COMMANDS
        .iter()
        .copied()
        .filter(move |(name, _, _)| scope.supports(name))
        .chain(phases)
}

/// The composition command that was typed without the brief it needs, if any.
pub(crate) fn missing_brief(text: &str) -> Option<&'static str> {
    PhaseCommand::parse(text)
        .filter(|(command, instruction)| command.needs_instruction() && instruction.is_empty())
        .map(|(command, _)| command.name())
}

const SLASH_COMMANDS: &[(&str, bool, Key)] = &[
    ("/mode", true, Key::AgentSlashMode),
    ("/model", true, Key::AgentSlashModel),
    ("/effort", true, Key::AgentSlashEffort),
    ("/permissions", false, Key::AgentSlashPermissions),
    ("/allow", true, Key::AgentSlashAllow),
    ("/deny", true, Key::AgentSlashDeny),
    ("/default", true, Key::AgentSlashDefault),
    ("/compact", false, Key::AgentSlashCompact),
    ("/help", false, Key::AgentSlashHelp),
    ("/read-only", false, Key::AgentSlashMode),
    ("/edit", false, Key::AgentSlashMode),
    ("/plan", false, Key::AgentSlashMode),
    ("/auto-review", false, Key::AgentSlashMode),
    ("/bypass", false, Key::AgentSlashMode),
];

/// Whether the text is handled locally before model and project-reload checks.
pub(crate) fn is_control_command(text: &str) -> bool {
    let command = text.split_once(' ').map_or(text, |(command, _)| command);
    SLASH_COMMANDS.iter().any(|(name, _, _)| *name == command)
}

pub(crate) fn slash_matches(value: &str, models: &[ModelOption], scope: Scope) -> Vec<SlashMatch> {
    if !value.starts_with('/') || value.contains('\n') {
        return Vec::new();
    }
    let Some((command, argument)) = value.split_once(' ') else {
        return catalogue(scope)
            .filter(|(name, _, _)| name.starts_with(value))
            .map(|(name, takes_argument, description)| SlashMatch {
                fill: format!("{name}{}", if takes_argument { " " } else { "" }),
                label: name.to_string(),
                description,
            })
            .collect();
    };
    if !scope.supports(command) {
        return Vec::new();
    }
    let argument = argument.trim_start();
    if command == "/model" {
        return model_search::matches(argument, models)
            .into_iter()
            .map(|index| SlashMatch {
                fill: format!("/model {}", model_search::command_name(index, models)),
                label: model_search::command_name(index, models),
                description: Key::AgentSlashModel,
            })
            .collect();
    }
    let (values, description): (Vec<&str>, Key) = match command {
        "/mode" => (
            vec!["read-only", "edit", "plan", "auto_review", "bypass"],
            Key::AgentSlashMode,
        ),
        "/effort" => (
            vec![
                "default", "none", "minimal", "low", "medium", "high", "xhigh", "max",
            ],
            Key::AgentSlashEffort,
        ),
        "/allow" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashAllow,
        ),
        "/deny" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashDeny,
        ),
        "/default" => (
            std::iter::once("*")
                .chain(std::iter::once("edit_project.*"))
                .chain(OPERATIONS.iter().copied())
                .collect(),
            Key::AgentSlashDefault,
        ),
        _ => return Vec::new(),
    };
    values
        .into_iter()
        .filter(|candidate| candidate.starts_with(argument))
        .map(|candidate| SlashMatch {
            fill: format!("{command} {candidate}"),
            label: candidate.to_string(),
            description,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn workshop_slash_catalogue_is_an_executable_subset_of_the_panel() {
        let workshop = slash_matches("/", &[], Scope::Workshop);
        let panel = slash_matches("/", &[], Scope::Agent);
        assert_eq!(
            workshop
                .iter()
                .map(|item| item.label.as_str())
                .collect::<Vec<_>>(),
            vec!["/model", "/effort", "/help"]
        );
        for item in workshop {
            assert!(is_control_command(item.fill.trim()));
            assert!(panel.iter().any(|other| other.fill == item.fill));
        }
        assert!(slash_matches("/mode ", &[], Scope::Workshop).is_empty());
        assert!(slash_matches("/allow ", &[], Scope::Workshop).is_empty());
    }

    #[test]
    fn shared_slash_arguments_complete_identically() {
        let models = vec![ModelOption {
            name: "org/lfm3-audio".into(),
            ..Default::default()
        }];
        for input in [
            "/model lfm3 auido",
            "/effort h",
            "/help",
            "/effort h\n",
            "write a chorus",
        ] {
            let fills = |scope| {
                slash_matches(input, &models, scope)
                    .into_iter()
                    .map(|item| item.fill)
                    .collect::<Vec<_>>()
            };
            assert_eq!(fills(Scope::Agent), fills(Scope::Workshop), "{input}");
        }
    }
}
