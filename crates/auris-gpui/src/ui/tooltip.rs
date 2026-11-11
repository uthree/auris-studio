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
use gpui::{AnyView, App, SharedString, Window};

impl crate::app::AurisApp {
    /// A tooltip naming one of the application's own controls (a readout, a strip, a lane
    /// mark), carrying whatever key currently runs it.
    ///
    /// An Atelier [`atelier::Button`] takes its words and keys itself, `.tooltip(..)` with
    /// `.shortcut(self.keystroke_for(..))`; this is for the elements Atelier does not draw.
    /// `command` is an id from [`crate::actions::BINDABLE`], or `""` for a control that is not
    /// a bindable command. An unknown id draws no keys rather than complaining, which is the
    /// same answer a command with no default key gets and is right for both.
    pub(crate) fn tip(
        &self,
        label: auris_i18n::Key,
        command: &str,
    ) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
        let label: SharedString = self.t(label).into();
        let keys: SharedString = self.keystroke_for(command).into();
        move |window, cx| {
            atelier::Tooltip::new(label.clone())
                .shortcut(keys.clone())
                .build(window, cx)
        }
    }
}

/// The callback gpui's `tooltip` wants for words with no keys: Atelier's tooltip card.
pub fn text_tip(
    label: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    tip_with_keys(label, "")
}

/// Atelier's tooltip card with the keys faint after the words, for an element of the
/// application's own that has a key or a gesture (a scrollbar's Shift+Wheel, a step's Space).
/// `keys` arrives already written for the platform; empty shows none.
pub fn tip_with_keys(
    label: impl Into<SharedString>,
    keys: impl Into<SharedString>,
) -> impl Fn(&mut Window, &mut App) -> AnyView + 'static {
    let label = label.into();
    let keys = keys.into();
    move |window, cx| {
        atelier::Tooltip::new(label.clone())
            .shortcut(keys.clone())
            .build(window, cx)
    }
}
