//! A hosted plugin's own window.
//!
//! # Floating, never embedded
//!
//! CLAP offers a host two ways to show a plugin: hand it a window of the host's to draw inside,
//! or let it make its own and float it above. Auris asks for the second, always.
//!
//! The first is the nicer one and it is not available here. Embedding means owning a native child
//! window — an `HWND` or an `NSView` — positioned inside the host's own, and gpui draws its entire
//! interface on one surface with no notion of a child window to give away. A plugin panel that
//! reserved a rectangle would be reserving a rectangle of a picture, and the plugin would draw
//! over the whole application.
//!
//! What is lost by floating is real and worth naming: the plugin's window is not part of the
//! layout, cannot be docked, and does not scroll with anything. What is kept is that it works at
//! all, on both platforms, without gpui growing a child-window API. [`set_transient`] is what
//! makes it bearable — the window is told which window to stay above, so it does not sink behind
//! the application the moment the application is clicked.
//!
//! [`set_transient`]: clack_extensions::gui::PluginGui::set_transient

use std::ffi::CString;

use clack_extensions::gui::{GuiApiType, GuiConfiguration};

/// How Auris will ask for every plugin window: this platform's own API, floating.
///
/// `None` on a platform CLAP has no window API for, which is every platform but the three the
/// specification names. A plugin is then edited through the parameter panel, as it was before any
/// of this existed.
pub fn window_plan() -> Option<GuiConfiguration<'static>> {
    Some(GuiConfiguration {
        api_type: GuiApiType::default_for_current_platform()?,
        is_floating: true,
    })
}

/// What to call a plugin's window.
///
/// The plugin's name first and the application's second. Somebody with four plugin windows open is
/// looking for *which plugin*, and a window list truncates the end — so the half that tells them
/// apart goes where it will survive. A plugin that gave no name gets the application's alone,
/// which is at least true.
pub fn window_title(plugin: &str) -> String {
    match plugin.trim() {
        "" => "Auris Studio".to_string(),
        name => format!("{name} — Auris Studio"),
    }
}

/// [`window_title`] as the C string `suggest_title` takes.
///
/// A name carrying a NUL byte cannot be passed on, and is not worth refusing to open a window
/// over: the title falls back to the application's own.
pub(crate) fn suggested_title(plugin: &str) -> CString {
    CString::new(window_title(plugin)).unwrap_or_else(|_| c"Auris Studio".to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_named_for_its_plugin_first() {
        assert_eq!(window_title("Surge XT"), "Surge XT — Auris Studio");
        assert_eq!(
            window_title("   "),
            "Auris Studio",
            "a plugin that gave no name does not get a window called ` — Auris Studio`"
        );
    }

    #[test]
    fn a_name_with_a_nul_in_it_costs_the_name_and_not_the_window() {
        assert_eq!(suggested_title("Su\0rge"), c"Auris Studio".to_owned());
        assert_eq!(suggested_title("Surge"), c"Surge — Auris Studio".to_owned());
    }

    #[test]
    fn every_window_this_host_asks_for_is_a_floating_one() {
        // Not a tautology: it is the one thing about the plan that could be quietly changed by
        // somebody reaching for embedding, and embedding is what the module doc explains gpui
        // cannot give.
        let plan = window_plan().expect("all three supported platforms have an API");
        assert!(plan.is_floating);
    }
}
