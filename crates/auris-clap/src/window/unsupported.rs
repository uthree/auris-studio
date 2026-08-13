//! The container window everywhere else, which is to say nowhere.
//!
//! Linux is the platform this is: X11 and Wayland both have a windowing API CLAP names, and
//! neither has a container here because Auris Studio does not ship there. Written as a value that
//! never exists rather than as a `#[cfg]` around every caller — a plugin that will only embed
//! reports no window it can open, which is the same answer the caller already handles.

use clack_extensions::gui::GuiSize;
use raw_window_handle::RawWindowHandle;

use super::ContainerWindow;

/// A window that is never opened.
pub(crate) enum HostWindow {}

impl ContainerWindow for HostWindow {
    fn open(_title: &str, _size: GuiSize, _parent: Option<RawWindowHandle>) -> Option<Self> {
        None
    }

    fn handle(&self) -> RawWindowHandle {
        match *self {}
    }

    fn scale(&self) -> f64 {
        match *self {}
    }

    fn resize(&self, _size: GuiSize) {
        match *self {}
    }

    fn show(&self) {
        match *self {}
    }

    fn was_closed(&self) -> bool {
        match *self {}
    }
}
