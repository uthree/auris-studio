//! The container window on macOS.

use std::ptr::NonNull;

use clack_extensions::gui::GuiSize;
use objc2::rc::Retained;
use objc2_app_kit::{
    NSBackingStoreType, NSView, NSWindow, NSWindowOrderingMode, NSWindowStyleMask,
};
use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};
use raw_window_handle::{AppKitWindowHandle, RawWindowHandle};

use super::{ContainerWindow, sane_size};

/// The plain window a plugin is embedded in.
pub(crate) struct HostWindow {
    window: Retained<NSWindow>,
    /// The window's own view, which is what the plugin is given and what it makes its own view a
    /// subview of. Held so the handle can be answered without asking the window each time.
    view: Retained<NSView>,
    /// Whether [`show`](ContainerWindow::show) has been called.
    ///
    /// A window is not visible before it is shown either, and without this the caller would be
    /// told the user had closed a window that had not been put up yet.
    shown: std::cell::Cell<bool>,
}

impl ContainerWindow for HostWindow {
    fn open(title: &str, size: GuiSize, parent: Option<RawWindowHandle>) -> Option<Self> {
        // Windows are the main thread's, and so is everything that reaches this: `ClapPlugin` is
        // not `Send`, and it is on the session's thread, which is the frontend's.
        let mtm = MainThreadMarker::new()?;

        let size = sane_size(size);
        let frame = NSRect::new(
            NSPoint::new(0.0, 0.0),
            NSSize::new(f64::from(size.width), f64::from(size.height)),
        );
        let style = NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable;

        // SAFETY: a plain window with a content rectangle and a style, on the main thread.
        let window = unsafe {
            NSWindow::initWithContentRect_styleMask_backing_defer(
                NSWindow::alloc(mtm),
                frame,
                style,
                NSBackingStoreType::Buffered,
                false,
            )
        };

        // Closing must not free the window: the plugin's own view is inside it and is still
        // holding on. Without this, the close box is a use-after-free rather than a close.
        // SAFETY: the window was made above and nothing else has it yet.
        unsafe { window.setReleasedWhenClosed(false) };
        window.setTitle(&NSString::from_str(title));
        window.center();

        let view = window.contentView()?;

        // Stays above the application's window and moves with it, which is what an owned window
        // does on Windows and what `set_transient` would have asked the plugin for.
        if let Some(owner) = parent.and_then(|handle| owning_window(handle, mtm)) {
            // SAFETY: both windows are alive; the child is the one just made.
            unsafe { owner.addChildWindow_ordered(&window, NSWindowOrderingMode::Above) };
        }

        Some(Self {
            window,
            view,
            shown: std::cell::Cell::new(false),
        })
    }

    fn handle(&self) -> RawWindowHandle {
        RawWindowHandle::AppKit(AppKitWindowHandle::new(
            NonNull::from(&*self.view).cast::<std::ffi::c_void>(),
        ))
    }

    fn scale(&self) -> f64 {
        // Cocoa's windowing API is already in logical pixels, and CLAP says so: a plugin told the
        // scale as well would apply it on top of the one the system is applying, and draw its
        // interface at the square of it.
        1.0
    }

    fn resize(&self, size: GuiSize) {
        let size = sane_size(size);
        self.window
            .setContentSize(NSSize::new(f64::from(size.width), f64::from(size.height)));
    }

    fn show(&self) {
        self.shown.set(true);
        self.window.makeKeyAndOrderFront(None);
    }

    fn was_closed(&self) -> bool {
        // No delegate, deliberately. Closing a window whose `releasedWhenClosed` is false orders
        // it out and leaves it standing, so "not visible any more" *is* the report — and it comes
        // without a subclass, an ivar and a lifetime to reason about.
        self.shown.get() && !self.window.isVisible()
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        self.window.close();
    }
}

/// The window a raw handle's view belongs to, if it is an AppKit one.
fn owning_window(handle: RawWindowHandle, _mtm: MainThreadMarker) -> Option<Retained<NSWindow>> {
    let RawWindowHandle::AppKit(handle) = handle else {
        return None;
    };
    // SAFETY: the caller vouched for the handle naming a live view — see `ClapPlugin::open_gui`,
    // which takes it from the application's own window.
    let view: &NSView = unsafe { handle.ns_view.cast::<NSView>().as_ref() };
    view.window()
}
