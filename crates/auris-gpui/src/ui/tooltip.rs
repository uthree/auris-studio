//! The card that says what an unlabelled control is, and which key also reaches it.
//!
//! A transport bar is a row of glyphs. A triangle is play everywhere and a circle is record
//! everywhere, but the punch, the click and the cycle are three rectangles with marks in them,
//! and nothing on screen said which was which — the only way to find out was to press one and
//! listen. The status bar's panel switches have the same shape of problem: five small marks, and
//! the panel each one opens is a thing you learn by clicking all five.
//!
//! The keystroke is on the card as well, and that is the half a tooltip in this application is
//! for. Every key here is the user's to move, so a printed sheet or a line in a manual would be
//! wrong for anybody who has been in the settings window; asking the keymap at the moment of
//! hovering is the only telling that stays true.
//!
//! # Why this is a view rather than an element
//!
//! gpui builds a tooltip from a callback returning an [`AnyView`], and calls it when the pointer
//! has rested. The view is built outside the tree that styles everything else, so it inherits no
//! font, no size and no colour and has to name all three. That is also why the theme is carried
//! into it: there is no ancestor to ask.

use gpui::{AnyView, App, Context, IntoElement, Render, SharedString, Window, div, prelude::*};

use crate::theme::{Metrics, Theme};

/// A control's name, over the keystroke that also runs it.
pub struct Tooltip {
    /// What the control is called.
    label: SharedString,
    /// The keystroke, written the way this platform writes it, or empty for a command with no
    /// key. Empty is common and is not a hole: most of what wears a tooltip here has no keystroke
    /// and never needed one.
    keystroke: SharedString,
    /// Carried in because a tooltip has no ancestor to inherit colours from.
    theme: Theme,
}

impl Render for Tooltip {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let theme = &self.theme;
        div()
            .flex()
            .items_center()
            .gap_2()
            .px_2()
            .py_1()
            .rounded(Metrics::RADIUS_MD)
            .bg(theme.surface_raised)
            .border_1()
            .border_color(theme.border)
            .shadow_lg()
            .font(crate::theme::ui_font())
            .text_xs()
            .text_color(theme.text)
            .child(self.label.clone())
            .when(!self.keystroke.is_empty(), |this| {
                this.child(
                    div()
                        .px_1()
                        .rounded(Metrics::RADIUS_SM)
                        .bg(theme.surface_sunken)
                        // Dimmer than the name, because the name is the answer and the keystroke
                        // is the footnote — and a chip in full-strength text reads as a second
                        // label rather than as a key.
                        .text_color(theme.text_muted)
                        .child(self.keystroke.clone()),
                )
            })
    }
}

impl crate::app::AurisApp {
    /// A tooltip naming a control, carrying whatever key currently runs it.
    ///
    /// `command` is an id from [`crate::actions::BINDABLE`], or `""` for a control that is not a
    /// bindable command — the stop button, a panel switch's neighbour, anything reached only by
    /// pressing it. An unknown id draws no chip rather than complaining, which is the same
    /// answer a command with no default key gets and is right for both.
    pub(crate) fn tip(
        &self,
        label: auris_i18n::Key,
        command: &str,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        keyed_tip(self.t(label), self.keystroke_for(command), &self.theme)
    }
}

/// The callback gpui's `tooltip` wants.
///
/// `keystroke` arrives already written the way this platform writes it — the caller has the
/// keymap and this does not. Empty means the command has no key, which is the ordinary case and
/// draws no chip.
pub fn keyed_tip(
    label: impl Into<SharedString>,
    keystroke: impl Into<SharedString>,
    theme: &Theme,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label = label.into();
    let keystroke = keystroke.into();
    let theme = theme.clone();
    move |_window, cx| {
        let tooltip = Tooltip {
            label: label.clone(),
            keystroke: keystroke.clone(),
            theme: theme.clone(),
        };
        cx.new(|_| tooltip).into()
    }
}
