//! Embed a plugin editor inside a host window (e.g. an egui/eframe window), as an
//! alternative to the standalone [`PluginWindow`](crate::PluginWindow).
//!
//! Instead of opening its own top-level OS window, the plugin's native editor view is
//! parented as a child of a window your UI framework already owns, positioned to track a
//! region you allocate. You provide the parent window's [`RawWindowHandle`] and a target
//! rectangle (in logical points, top-left origin — the egui convention) each frame.
//!
//! Implemented on macOS (verified), Windows, and Linux/X11. A Wayland `RawWindowHandle` is
//! rejected explicitly: VST 3.8 requires the host to provide a compositor connection through
//! `IWaylandHost`, not merely pass the application's system-compositor `wl_surface`. Other
//! platforms return an error from [`EmbeddedEditor::embed`]. Requires the `egui-widgets` feature.
//!
//! Sizing goes both ways. The host proposes a size through [`EmbeddedEditor::set_rect`] and the
//! plugin may adjust or refuse it; the plugin proposes one through
//! [`EmbeddedEditor::take_resize_request`], which the host should poll each frame and answer by
//! allocating that much space.
//!
//! On Windows the parent window and its message loop remain owned by the UI framework, so this
//! type cannot intercept `WM_DPICHANGED`. Forward framework scale-factor changes explicitly with
//! [`Plugin::set_editor_scale_factor`](crate::Plugin::set_editor_scale_factor); the initial child
//! DPI is communicated automatically when embedding.
#![cfg(feature = "egui-widgets")]

use crate::error::{Error, Result};
use crate::plugin::Plugin;
use raw_window_handle::RawWindowHandle;
use std::cell::Cell;
use std::sync::{Arc, Mutex};

// The native child view/window the plugin's editor is parented into, per platform. All three
// expose the same `new` / `set_rect` pair, so the cross-platform code below names only this.
#[cfg(target_os = "linux")]
use linux::LinuxEmbed as PlatformEmbed;
#[cfg(target_os = "macos")]
use macos::MacEmbed as PlatformEmbed;
#[cfg(target_os = "windows")]
use windows::WinEmbed as PlatformEmbed;

/// A rectangle in the host view's logical points, **top-left origin** (egui convention).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EditorRect {
    /// Left edge, points from the window's left.
    pub x: f32,
    /// Top edge, points from the window's top.
    pub y: f32,
    /// Width in points.
    pub width: f32,
    /// Height in points.
    pub height: f32,
}

/// The most recent size negotiation with the plugin: what the host asked for, and what the
/// plugin's `checkSizeConstraint` answered (or the fallback used when it refused outright).
#[derive(Clone, Copy)]
struct NegotiatedSize {
    requested: (i32, i32),
    accepted: (i32, i32),
}

/// A plugin editor embedded into a host window. Drop it (or call [`Self::close`]) to detach
/// the editor and remove the child view.
///
/// Not `Sync`: it caches the last negotiated size in a [`Cell`], and every method belongs on
/// the UI thread that owns the parent window anyway.
pub struct EmbeddedEditor {
    plugin: Arc<Mutex<Plugin>>,
    /// Cached so a host that calls [`Self::set_rect`] every frame does not send the plugin an
    /// `onSize` for a size it already answered.
    negotiated: Cell<Option<NegotiatedSize>>,
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    inner: PlatformEmbed,
}

impl EmbeddedEditor {
    /// Embed `plugin`'s editor as a child of `parent`, at `rect`.
    ///
    /// Must be called on the UI/main thread (where your event loop runs). `parent` is the
    /// host window's handle (e.g. from `eframe::Frame::window_handle()`).
    ///
    /// On Windows, continue forwarding later DPI/scale changes through
    /// [`Plugin::set_editor_scale_factor`](crate::Plugin::set_editor_scale_factor). An embedded
    /// child does not own the parent framework's window procedure.
    ///
    /// Sizing the editor to `rect` is best-effort: a fixed-size editor, or one behind process
    /// isolation (where resize requests are not marshalled), keeps its own size and the embed
    /// still succeeds. Read the size the child actually got back from
    /// [`Self::try_set_rect`].
    pub fn embed(
        plugin: Arc<Mutex<Plugin>>,
        parent: RawWindowHandle,
        rect: EditorRect,
    ) -> Result<Self> {
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        {
            let inner = PlatformEmbed::new(&plugin, parent, rect)?;
            Ok(Self::sized(plugin, inner, rect))
        }
        #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
        {
            let _ = (&plugin, parent, rect);
            Err(Error::Other(
                "editor embedding is not implemented on this platform".to_string(),
            ))
        }
    }

    /// Assemble the editor and apply the caller's initial rectangle, tolerating a plugin that
    /// declines the size — the view is attached either way, so failing here would throw away a
    /// working editor over a cosmetic mismatch.
    #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
    fn sized(plugin: Arc<Mutex<Plugin>>, inner: PlatformEmbed, rect: EditorRect) -> Self {
        let editor = Self {
            plugin,
            negotiated: Cell::new(None),
            inner,
        };
        if let Err(error) = editor.try_set_rect(rect) {
            log::warn!(
                "embedded editor kept its own size, the requested one was declined: {error} \
                 (call EmbeddedEditor::try_set_rect to read the size in effect)"
            );
        }
        editor
    }

    /// Reposition/resize the embedded editor to track `rect`. Call each frame so the editor
    /// follows the host layout (scroll, window resize).
    ///
    /// The position always follows `rect`. The *size* is whatever the plugin accepts: a
    /// fixed-size editor keeps its own dimensions, so the child view may be smaller or larger
    /// than the rectangle you asked for. Use [`Self::try_set_rect`] to learn which.
    ///
    /// Implemented on macOS, Windows and Linux/X11 — the same platforms
    /// [`Self::embed`] supports. A no-op on any other platform, where `embed` cannot have
    /// succeeded in the first place.
    pub fn set_rect(&self, rect: EditorRect) {
        let _ = self.try_set_rect(rect);
    }

    /// Fallible variant of [`Self::set_rect`] that reports a rejected resize.
    ///
    /// On success the returned rectangle carries the dimensions the plugin accepted after
    /// applying its `checkSizeConstraint` rules, which may be smaller or larger than the ones
    /// requested. On failure the child view has still been moved to `rect`'s position and kept
    /// at the last size the plugin accepted — a plugin that refuses to resize does not stop the
    /// editor from tracking the host layout.
    ///
    /// Repeating the same size is cheap: the plugin is only asked when the requested dimensions
    /// differ from the last ones it answered.
    pub fn try_set_rect(&self, rect: EditorRect) -> Result<EditorRect> {
        let requested = validated_size(rect)?;

        // Already negotiated: move the child, skip the COM round-trip. Host layouts call this
        // every frame and an `onSize` per frame is spam the plugin has to redraw for.
        if let Some(previous) = self.negotiated.get() {
            if previous.requested == requested {
                return Ok(self.place(rect, previous.accepted));
            }
        }

        // VST 3 order: the container the view lives in takes its new size first, then the view
        // is told about it (`IPlugView::onSize`, via `Plugin::resize_editor`).
        self.place(rect, requested);

        let outcome = self.negotiate(requested);
        let accepted = match &outcome {
            Ok(accepted) => *accepted,
            // Refusal is not fatal. Fall back to the size the plugin last accepted, else the
            // size it reports for itself, else leave the container where we just put it.
            Err(_) => self.fallback_size().unwrap_or(requested),
        };
        self.negotiated.set(Some(NegotiatedSize {
            requested,
            accepted,
        }));

        let placed = if accepted == requested {
            EditorRect {
                width: requested.0 as f32,
                height: requested.1 as f32,
                ..rect
            }
        } else {
            self.place(rect, accepted)
        };
        outcome.map(|_| placed)
    }

    /// Poll for a resize the *plugin* asked for through VST 3's `IPlugFrame::resizeView`, in
    /// pixels, consuming it.
    ///
    /// An embedded editor cannot resize the host's layout on its own. Call this each frame
    /// while the editor is open; when it answers, allocate that much space in your UI and feed
    /// the resulting rectangle back through [`Self::set_rect`]. Ignoring it leaves the plugin's
    /// view drawing at a size its container does not match (clipped or letterboxed).
    ///
    /// Returns `None` when nothing is pending, when the plugin mutex is momentarily held
    /// elsewhere (the request stays queued for the next poll), and always for a
    /// process-isolated plugin, whose editor is not bridged across the boundary.
    pub fn take_resize_request(&self) -> Option<(i32, i32)> {
        try_lock(&self.plugin)?.take_editor_resize_request()
    }

    /// Ask the plugin to accept `size`, reporting what it settled on.
    fn negotiate(&self, (width, height): (i32, i32)) -> Result<(i32, i32)> {
        self.plugin
            .lock()
            .map_err(|_| Error::Other("plugin lock poisoned".to_string()))?
            .resize_editor(width, height)
    }

    /// The size to keep when the plugin refuses a resize.
    fn fallback_size(&self) -> Option<(i32, i32)> {
        if let Some(previous) = self.negotiated.get() {
            return Some(previous.accepted);
        }
        try_lock(&self.plugin)?.get_editor_size().ok()
    }

    /// Move the native child to `rect`'s position at `size`, and report the rectangle applied.
    fn place(&self, rect: EditorRect, size: (i32, i32)) -> EditorRect {
        let placed = EditorRect {
            width: size.0 as f32,
            height: size.1 as f32,
            ..rect
        };
        // The plugin lock is deliberately not held here. On Windows, resizing the HWND may
        // synchronously dispatch window messages back into host code.
        #[cfg(any(target_os = "macos", target_os = "windows", target_os = "linux"))]
        self.inner.set_rect(placed);
        placed
    }

    /// Detach the editor and remove the child view (also done on drop).
    pub fn close(self) {}
}

/// Reject rectangles a native window system cannot represent, and round the size to pixels.
fn validated_size(rect: EditorRect) -> Result<(i32, i32)> {
    let finite = rect.x.is_finite()
        && rect.y.is_finite()
        && rect.width.is_finite()
        && rect.height.is_finite();
    if !finite
        || rect.width <= 0.0
        || rect.height <= 0.0
        || rect.width > i32::MAX as f32
        || rect.height > i32::MAX as f32
    {
        return Err(Error::Other(
            "embedded editor rectangle must be finite with positive dimensions".to_string(),
        ));
    }
    Ok((rect.width.round() as i32, rect.height.round() as i32))
}

/// Take the plugin lock without blocking, recovering a lock poisoned by an unrelated panic.
///
/// Used by the per-frame polls: the audio callback holds this mutex for each block, and
/// stalling the UI thread behind it would stutter the host. Missing a frame costs nothing —
/// both callers retry.
fn try_lock(plugin: &Mutex<Plugin>) -> Option<std::sync::MutexGuard<'_, Plugin>> {
    match plugin.try_lock() {
        Ok(guard) => Some(guard),
        Err(std::sync::TryLockError::Poisoned(poison)) => Some(poison.into_inner()),
        Err(std::sync::TryLockError::WouldBlock) => None,
    }
}

#[cfg(test)]
mod rect_tests {
    use super::*;

    fn rect(width: f32, height: f32) -> EditorRect {
        EditorRect {
            x: 4.0,
            y: 8.0,
            width,
            height,
        }
    }

    #[test]
    fn rounds_the_requested_size_to_whole_pixels() {
        assert_eq!(validated_size(rect(799.4, 600.5)).unwrap(), (799, 601));
    }

    #[test]
    fn rejects_sizes_a_window_system_cannot_represent() {
        for bad in [
            rect(0.0, 600.0),
            rect(800.0, -1.0),
            rect(f32::NAN, 600.0),
            rect(800.0, f32::INFINITY),
        ] {
            assert!(
                validated_size(bad).is_err(),
                "{bad:?} should not reach the plugin"
            );
        }
    }

    #[test]
    fn rejects_a_non_finite_position_even_with_a_valid_size() {
        let mut bad = rect(800.0, 600.0);
        bad.x = f32::NAN;
        assert!(validated_size(bad).is_err());
    }
}

impl Drop for EmbeddedEditor {
    fn drop(&mut self) {
        // Detach the plugin's view first; the platform child view is torn down by `inner`.
        if let Ok(mut p) = self.plugin.lock() {
            let _ = p.close_editor();
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use objc2::{rc::Retained, MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::NSView;
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    pub struct MacEmbed {
        parent: Retained<NSView>,
        child: Retained<NSView>,
    }

    impl MacEmbed {
        pub fn new(
            plugin: &Arc<Mutex<Plugin>>,
            parent: RawWindowHandle,
            rect: EditorRect,
        ) -> Result<Self> {
            let mtm = MainThreadMarker::new().ok_or_else(|| {
                Error::Other("editor embedding must run on the main thread".to_string())
            })?;
            let RawWindowHandle::AppKit(h) = parent else {
                return Err(Error::Other(
                    "expected an AppKit window handle for the parent".to_string(),
                ));
            };
            // The host owns `ns_view`; retain it so it outlives our use.
            let parent: Retained<NSView> =
                unsafe { Retained::retain(h.ns_view.as_ptr() as *mut NSView) }
                    .ok_or_else(|| Error::Other("null parent NSView".to_string()))?;

            // Create the container child view the plugin attaches into.
            let frame = NSRect::new(
                NSPoint::new(rect.x as f64, 0.0),
                NSSize::new(rect.width as f64, rect.height as f64),
            );
            let child = NSView::initWithFrame(NSView::alloc(mtm), frame);
            parent.addSubview(&child);

            // SAFETY: `child` is a live NSView, retained by this `MacEmbed` for as long as the
            // editor is attached — `Drop` removes it from its superview only after
            // `EmbeddedEditor::drop` has closed the editor.
            let handle = unsafe {
                crate::plugin::WindowHandle::from_nsview(
                    Retained::as_ptr(&child) as *mut std::ffi::c_void
                )
            };
            plugin
                .lock()
                .map_err(|_| Error::Other("plugin lock poisoned".to_string()))?
                .open_editor(handle)?;

            Ok(Self { parent, child })
        }

        pub fn set_rect(&self, rect: EditorRect) {
            // Convert egui's top-left origin to the parent view's coordinate space. AppKit
            // views are bottom-left origin unless flipped, so flip Y against the parent's
            // current height (which changes as the window resizes).
            let flipped = self.parent.isFlipped();
            let parent_height = self.parent.bounds().size.height;
            let y = if flipped {
                rect.y as f64
            } else {
                parent_height - (rect.y + rect.height) as f64
            };
            let frame = NSRect::new(
                NSPoint::new(rect.x as f64, y),
                NSSize::new(rect.width as f64, rect.height as f64),
            );
            self.child.setFrame(frame);
        }
    }

    impl Drop for MacEmbed {
        fn drop(&mut self) {
            self.child.removeFromSuperview();
        }
    }
}

#[cfg(target_os = "windows")]
mod windows {
    use super::*;
    use winapi::shared::windef::HWND;
    use winapi::um::libloaderapi::GetModuleHandleW;
    use winapi::um::winuser::{
        CreateWindowExW, DefWindowProcW, DestroyWindow, RegisterClassExW, SetWindowPos, ShowWindow,
        CS_HREDRAW, CS_VREDRAW, SWP_NOZORDER, SW_SHOW, WNDCLASSEXW, WS_CHILD, WS_VISIBLE,
    };

    /// A plugin editor embedded as a child `HWND` of the host window.
    pub struct WinEmbed {
        child: HWND,
    }

    impl WinEmbed {
        pub fn new(
            plugin: &Arc<Mutex<Plugin>>,
            parent: RawWindowHandle,
            rect: EditorRect,
        ) -> Result<Self> {
            let RawWindowHandle::Win32(h) = parent else {
                return Err(Error::Other(
                    "expected a Win32 window handle for the parent".to_string(),
                ));
            };
            unsafe {
                let parent_hwnd = h.hwnd.get() as HWND;
                let hinstance = GetModuleHandleW(std::ptr::null());

                // Register a child window class (idempotent across calls).
                let class_name: Vec<u16> = "VST3EmbeddedEditor\0".encode_utf16().collect();
                let mut wc: WNDCLASSEXW = std::mem::zeroed();
                wc.cbSize = std::mem::size_of::<WNDCLASSEXW>() as u32;
                wc.style = CS_HREDRAW | CS_VREDRAW;
                wc.lpfnWndProc = Some(DefWindowProcW);
                wc.hInstance = hinstance;
                wc.lpszClassName = class_name.as_ptr();
                RegisterClassExW(&wc);

                let child = CreateWindowExW(
                    0,
                    class_name.as_ptr(),
                    std::ptr::null(),
                    WS_CHILD | WS_VISIBLE,
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as i32,
                    rect.height as i32,
                    parent_hwnd,
                    std::ptr::null_mut(),
                    hinstance,
                    std::ptr::null_mut(),
                );
                if child.is_null() {
                    return Err(Error::Other("Failed to create child window".to_string()));
                }

                // SAFETY: `child` was just created above and null-checked; it is destroyed only
                // after the editor is detached (the error arm below, or `Drop`).
                let handle = crate::plugin::WindowHandle::from_hwnd(child as *mut std::ffi::c_void);
                let mut plugin = plugin
                    .lock()
                    .map_err(|_| Error::Other("plugin lock poisoned".to_string()))?;
                let dpi = winapi::um::winuser::GetDpiForWindow(child);
                if dpi > 0 {
                    if let Err(error) = plugin.set_editor_scale_factor(dpi as f32 / 96.0) {
                        drop(plugin);
                        DestroyWindow(child);
                        return Err(error);
                    }
                }
                if let Err(e) = plugin.open_editor(handle) {
                    drop(plugin);
                    DestroyWindow(child);
                    return Err(e);
                }
                drop(plugin);
                ShowWindow(child, SW_SHOW);
                Ok(Self { child })
            }
        }

        pub fn set_rect(&self, rect: EditorRect) {
            unsafe {
                SetWindowPos(
                    self.child,
                    std::ptr::null_mut(),
                    rect.x as i32,
                    rect.y as i32,
                    rect.width as i32,
                    rect.height as i32,
                    SWP_NOZORDER,
                );
            }
        }
    }

    impl Drop for WinEmbed {
        fn drop(&mut self) {
            unsafe {
                DestroyWindow(self.child);
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use xcb::{x, Xid, XidNew};

    const WAYLAND_UNSUPPORTED: &str = "Wayland VST3 editor embedding requires a host compositor \
        plus IWaylandHost/IWaylandFrame; a RawWindowHandle supplies only the system-compositor \
        wl_surface, so this host cannot attach it safely";

    fn x11_parent_id(parent: RawWindowHandle) -> Result<u32> {
        match parent {
            RawWindowHandle::Xcb(handle) => Ok(handle.window.get()),
            RawWindowHandle::Xlib(handle) => u32::try_from(handle.window)
                .map_err(|_| Error::Other("Xlib parent window id exceeds 32 bits".to_string())),
            RawWindowHandle::Wayland(_) => Err(Error::Other(WAYLAND_UNSUPPORTED.to_string())),
            _ => Err(Error::Other(
                "expected an X11 (Xcb/Xlib) window handle for the parent".to_string(),
            )),
        }
    }

    /// A plugin editor embedded as a child X11 window of the host window.
    pub struct LinuxEmbed {
        connection: xcb::Connection,
        child: x::Window,
    }

    impl LinuxEmbed {
        pub fn new(
            plugin: &Arc<Mutex<Plugin>>,
            parent: RawWindowHandle,
            rect: EditorRect,
        ) -> Result<Self> {
            let parent_id = x11_parent_id(parent)?;

            let (connection, screen_number) = xcb::Connection::connect(None)
                .map_err(|e| Error::Other(format!("Failed to connect to X server: {e}")))?;
            let visual = {
                let setup = connection.get_setup();
                let screen = setup
                    .roots()
                    .nth(screen_number as usize)
                    .ok_or_else(|| Error::Other("No X11 screen found".to_string()))?;
                screen.root_visual()
            };
            // `parent_id` is a live X11 window id from the host's RawWindowHandle.
            let parent_win: x::Window = x::Window::new(parent_id);
            let child = connection.generate_id();

            connection
                .send_and_check_request(&x::CreateWindow {
                    depth: x::COPY_FROM_PARENT as u8,
                    wid: child,
                    parent: parent_win,
                    x: rect.x as i16,
                    y: rect.y as i16,
                    width: (rect.width as u16).max(1),
                    height: (rect.height as u16).max(1),
                    border_width: 0,
                    class: x::WindowClass::InputOutput,
                    visual,
                    value_list: &[x::Cw::EventMask(x::EventMask::EXPOSURE)],
                })
                .map_err(|e| Error::Other(format!("Failed to create X11 child window: {e}")))?;
            connection.send_request(&x::MapWindow { window: child });
            let _ = connection.flush();

            let handle = crate::plugin::WindowHandle::from_x11(child.resource_id());
            if let Err(e) = plugin
                .lock()
                .map_err(|_| Error::Other("plugin lock poisoned".to_string()))?
                .open_editor(handle)
            {
                connection.send_request(&x::DestroyWindow { window: child });
                let _ = connection.flush();
                return Err(e);
            }

            Ok(Self { connection, child })
        }

        pub fn set_rect(&self, rect: EditorRect) {
            self.connection.send_request(&x::ConfigureWindow {
                window: self.child,
                value_list: &[
                    x::ConfigWindow::X(rect.x as i32),
                    x::ConfigWindow::Y(rect.y as i32),
                    x::ConfigWindow::Width((rect.width as u32).max(1)),
                    x::ConfigWindow::Height((rect.height as u32).max(1)),
                ],
            });
            let _ = self.connection.flush();
        }
    }

    impl Drop for LinuxEmbed {
        fn drop(&mut self) {
            self.connection
                .send_request(&x::DestroyWindow { window: self.child });
            let _ = self.connection.flush();
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use raw_window_handle::{WaylandWindowHandle, XcbWindowHandle};
        use std::num::NonZeroU32;
        use std::ptr::NonNull;

        #[test]
        fn accepts_x11_parent_without_touching_the_x_server() {
            let handle = XcbWindowHandle::new(NonZeroU32::new(73).unwrap());
            assert_eq!(x11_parent_id(RawWindowHandle::Xcb(handle)).unwrap(), 73);
        }

        #[test]
        fn rejects_wayland_surface_with_actionable_contract_error() {
            let surface = NonNull::<u8>::dangling().cast();
            let handle = WaylandWindowHandle::new(surface);
            let error = x11_parent_id(RawWindowHandle::Wayland(handle)).unwrap_err();
            assert!(error.to_string().contains("IWaylandHost/IWaylandFrame"));
            assert!(error.to_string().contains("RawWindowHandle"));
        }
    }
}
