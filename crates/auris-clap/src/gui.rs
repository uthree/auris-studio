//! A hosted plugin's own window.
//!
//! # Floating first, embedded when it has to be
//!
//! CLAP offers a host two ways to show a plugin: let it make its own window and float it above the
//! application, or hand it a window of the host's to draw inside. A plugin says which of the two
//! it can do, and it is allowed to say only one.
//!
//! Floating is the one to prefer, because everything about the window is then the plugin's problem
//! rather than a second implementation of window management living here. It is also not what most
//! plugins offer. Every plugin built on JUCE — which is most of them, including Surge XT — draws
//! only into a window it is given:
//!
//! ```text
//! Surge XT Effects.clap  offers: ["embedded"]
//! ```
//!
//! So the host has to be able to provide one, and `auris_clap::window` is the plain native window
//! it provides. What it is *not* is a panel inside the application: gpui draws its whole
//! interface onto one surface and has no child window to give away, so a panel reserving a
//! rectangle would be reserving a rectangle of a picture.
//!
//! Either way the plugin ends up in a window of its own, floating above the application. The
//! difference is only who made it — which is why [`set_transient`] and an owned native window are
//! the same idea reached from two directions.
//!
//! [`set_transient`]: clack_extensions::gui::PluginGui::set_transient

use std::ffi::CString;

use clack_extensions::gui::{GuiApiType, GuiConfiguration};

/// How to ask for a window, given what the plugin says it can do.
///
/// Floating wins wherever it is offered. A plugin that manages its own window has already solved
/// the parts this host would otherwise have to — where it opens, what happens when the display
/// changes, how it behaves full screen — and a host doing that work again is a host with two
/// window implementations, one of which is exercised by a minority of plugins.
///
/// `None` means the plugin has no window this host can put up: it offers neither, or it drew the
/// short straw of a platform CLAP names no windowing API for.
pub fn plan_for(floating: bool, embedded: bool) -> Option<GuiConfiguration<'static>> {
    let api_type = GuiApiType::default_for_current_platform()?;
    match (floating, embedded) {
        (true, _) => Some(GuiConfiguration {
            api_type,
            is_floating: true,
        }),
        (false, true) => Some(GuiConfiguration {
            api_type,
            is_floating: false,
        }),
        (false, false) => None,
    }
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
    fn a_plugin_gets_the_kind_of_window_it_can_actually_use() {
        // The bug this fixes: the host asked for floating and nothing else, so Surge XT — which
        // offers only embedding, like everything built on JUCE — reported no window at all and
        // the button to open one never appeared.
        assert!(
            plan_for(true, true).expect("both").is_floating,
            "floating wherever it is offered, so the plugin manages its own window"
        );
        assert!(plan_for(true, false).expect("floating only").is_floating);
        assert!(
            !plan_for(false, true).expect("embedded only").is_floating,
            "and a window of ours when that is the only thing the plugin can draw in"
        );
        assert!(plan_for(false, false).is_none(), "no window to open");
    }
}
