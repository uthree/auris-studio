//! A bare window for a plugin that will only draw inside one of the host's.
//!
//! # Why the host has to own a window at all
//!
//! CLAP lets a plugin offer a floating window it manages itself, an embedded one it draws inside a
//! window of the host's, or both. Auris asked for floating and nothing else, on the assumption
//! that a plugin would always offer it. Surge XT says otherwise:
//!
//! ```text
//! Surge XT Effects.clap  offers: ["embedded"]
//! ```
//!
//! and so does every other plugin built on JUCE, which is most of them. They do not create windows;
//! they draw into one they are given. A host with nothing to give them shows nothing.
//!
//! # Why it is not a gpui window
//!
//! Embedding means handing over a native window — an `HWND` or an `NSView` — for the plugin to
//! make its own a child of. gpui draws its whole interface onto one surface and has no child
//! window to give away, so a *panel* is out; that much was already true. A second gpui *window*
//! looks like it would work and does not: on Windows gpui presents through a flip-model swap
//! chain, and a child window over one of those is composited away rather than drawn.
//!
//! So this is a plain window and nothing else. It has a title bar, it is owned by the
//! application's window so it stays above it, and it paints nothing — the plugin's own window
//! covers every pixel of it. There is no renderer, no layout and no event handling beyond
//! noticing that somebody closed it.
//!
//! # Closing
//!
//! Both platforms report a close by *not destroying anything*. The plugin's window is a child of
//! this one, and pulling it out from under code that has not been told yet is the crash this
//! avoids: the window is left standing, a flag is set, and the owner tears the two down in the
//! right order — plugin first — when it next looks.

use clack_extensions::gui::GuiSize;
use raw_window_handle::RawWindowHandle;

#[cfg(target_os = "windows")]
mod win32;
#[cfg(target_os = "windows")]
pub(crate) use win32::HostWindow;

#[cfg(target_os = "macos")]
mod cocoa;
#[cfg(target_os = "macos")]
pub(crate) use cocoa::HostWindow;

#[cfg(not(any(target_os = "windows", target_os = "macos")))]
mod unsupported;
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) use unsupported::HostWindow;

/// Whether this platform can lend a plugin a window at all.
///
/// Read before a plugin is offered an embedded window rather than after it has failed to get one:
/// a host that answers "yes, there is a window" and then cannot make one has already put a button
/// on screen that does nothing.
pub(crate) const CAN_LEND: bool = cfg!(any(target_os = "windows", target_os = "macos"));

/// The size a container is made at, before the plugin has been asked what it wants.
///
/// It exists for one moment: a window has to be on screen to be asked what monitor it is on, the
/// answer decides the scale, and the scale decides the size the plugin then reports. Nothing is
/// ever shown at this size.
pub(crate) const PROVISIONAL: GuiSize = GuiSize {
    width: 400,
    height: 300,
};

/// A size a plugin's window is allowed to be.
///
/// A plugin that answers `get_size` with nonsense — zero, or a number that would fill a wall —
/// gets a window it can still be found and closed at. Both ends are deliberately generous: a
/// modern synth's interface really is fifteen hundred pixels across, and clamping a real answer is
/// worse than passing on a strange one.
// On the platforms that lend no window nothing calls this, but the rule and its test hold
// everywhere — allowed dead there rather than gated out with them.
#[cfg_attr(not(any(target_os = "windows", target_os = "macos")), allow(dead_code))]
pub(crate) fn sane_size(size: GuiSize) -> GuiSize {
    GuiSize {
        width: size.width.clamp(120, 8192),
        height: size.height.clamp(80, 8192),
    }
}

/// What every platform's container has to be able to do.
///
/// Not a trait — there is exactly one implementation compiled at a time, and a trait would only
/// make it harder to see that. This is the list, kept here so the platform files can be read
/// against it.
#[allow(dead_code)]
pub(crate) trait ContainerWindow: Sized {
    /// Opens a window with a client area of `size`, owned by `parent` so it stays above it.
    ///
    /// `None` when the platform will not give one, which is not a failure worth a message of its
    /// own: the caller has a plugin it cannot show either way.
    fn open(title: &str, size: GuiSize, parent: Option<RawWindowHandle>) -> Option<Self>;

    /// The handle to hand the plugin as the window to draw inside.
    fn handle(&self) -> RawWindowHandle;

    /// How many physical pixels this window has per logical one.
    ///
    /// `1.0` where the platform's windowing API is already in logical pixels, which is what CLAP
    /// says to do on Cocoa: asking a plugin to scale itself when the system is already doing it
    /// gives an interface at the square of the scale.
    fn scale(&self) -> f64;

    /// Resizes the client area, leaving the window where it is.
    fn resize(&self, size: GuiSize);

    /// Puts it on screen. Called once the plugin is drawing in it, and not before — a window that
    /// appears empty and fills a moment later reads as a flicker.
    fn show(&self);

    /// Whether the user has closed it.
    fn was_closed(&self) -> bool;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_window_is_never_made_too_small_to_close() {
        // A plugin answering `get_size` with zero is not hypothetical: it is what several answer
        // before their editor has been created once. A window with no title bar left is one the
        // user cannot get rid of.
        let tiny = sane_size(GuiSize {
            width: 0,
            height: 0,
        });
        assert!(tiny.width >= 120 && tiny.height >= 80);

        // And a real answer passes through untouched, which is the case that matters every other
        // time.
        let real = GuiSize {
            width: 1183,
            height: 700,
        };
        assert_eq!(sane_size(real), real);
    }
}
