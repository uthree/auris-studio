//! The container window on Windows.

use std::num::NonZeroIsize;
use std::sync::Arc;
use std::sync::Once;
use std::sync::atomic::{AtomicBool, Ordering};

use clack_extensions::gui::GuiSize;
use raw_window_handle::{RawWindowHandle, Win32WindowHandle};
use windows::Win32::Foundation::{HWND, LPARAM, LRESULT, RECT, WPARAM};
use windows::Win32::Graphics::Gdi::{BLACK_BRUSH, GetStockObject, HBRUSH};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::GetDpiForWindow;
use windows::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW,
    DestroyWindow, GWLP_USERDATA, GetWindowLongPtrW, HMENU, IDC_ARROW, LoadCursorW,
    RegisterClassExW, SW_SHOWNORMAL, SWP_NOMOVE, SWP_NOZORDER, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, WINDOW_EX_STYLE, WM_CLOSE, WNDCLASSEXW, WS_CAPTION, WS_MINIMIZEBOX, WS_OVERLAPPED,
    WS_SYSMENU,
};
use windows::core::{HSTRING, PCWSTR, w};

use super::{ContainerWindow, sane_size};

/// What Windows knows the container by. Registered once for the process.
const CLASS: PCWSTR = w!("AurisStudioPluginWindow");

/// The plain window a plugin is embedded in.
pub(crate) struct HostWindow {
    hwnd: HWND,
    /// Set by the window procedure when somebody closes the window.
    ///
    /// An [`Arc`] because the window procedure reaches it through a pointer stored on the window
    /// itself, and that pointer has to stay good for exactly as long as the window does. Dropping
    /// destroys the window first and this second.
    closed: Arc<AtomicBool>,
}

impl ContainerWindow for HostWindow {
    fn open(title: &str, size: GuiSize, parent: Option<RawWindowHandle>) -> Option<Self> {
        register_class();

        let size = sane_size(size);
        let style = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX;
        let frame = frame_for(size, style)?;

        // The owner, not the parent: passing a window here with a style that is not `WS_CHILD`
        // makes this an *owned* window, which stays above the one that owns it and goes away with
        // it. That is `set_transient` without asking the plugin for it.
        let owner = parent.and_then(hwnd_of).unwrap_or_default();
        let closed = Arc::new(AtomicBool::new(false));

        // SAFETY: the class is registered above, the title outlives the call, and the owner is
        // either a window handle the caller vouched for or null.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                CLASS,
                &HSTRING::from(title),
                style,
                CW_USEDEFAULT,
                CW_USEDEFAULT,
                frame.0,
                frame.1,
                Some(owner),
                None::<HMENU>,
                Some(GetModuleHandleW(None).ok()?.into()),
                None,
            )
        }
        .ok()?;

        // SAFETY: `closed` is owned by the value being built and outlives the window, which is
        // destroyed in `Drop` before the `Arc` is.
        unsafe { SetWindowLongPtrW(hwnd, GWLP_USERDATA, Arc::as_ptr(&closed) as isize) };

        Some(Self { hwnd, closed })
    }

    fn handle(&self) -> RawWindowHandle {
        // PANIC: a window handle is never null — `CreateWindowExW` returning one is the failure
        // `open` already turned into `None`.
        RawWindowHandle::Win32(Win32WindowHandle::new(
            NonZeroIsize::new(self.hwnd.0 as isize).expect("a live window is not the null handle"),
        ))
    }

    fn scale(&self) -> f64 {
        // Win32 windows are in physical pixels, so the plugin has to be told what a logical one is
        // worth here or it draws its interface at a quarter size on a retina display.
        // SAFETY: the window is alive for as long as this value is.
        let dpi = unsafe { GetDpiForWindow(self.hwnd) };
        match dpi {
            0 => 1.0,
            dpi => f64::from(dpi) / 96.0,
        }
    }

    fn resize(&self, size: GuiSize) {
        let Some(frame) = frame_for(sane_size(size), style_of(self.hwnd)) else {
            return;
        };
        // SAFETY: the window is alive for as long as this value is.
        let _ = unsafe {
            SetWindowPos(
                self.hwnd,
                None,
                0,
                0,
                frame.0,
                frame.1,
                SWP_NOMOVE | SWP_NOZORDER,
            )
        };
    }

    fn show(&self) {
        // SAFETY: the window is alive for as long as this value is.
        let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNORMAL) };
    }

    fn was_closed(&self) -> bool {
        self.closed.load(Ordering::Acquire)
    }
}

impl Drop for HostWindow {
    fn drop(&mut self) {
        // The pointer goes first: a message arriving between here and the window being gone would
        // otherwise reach an `Arc` that is about to be dropped.
        // SAFETY: the window is alive until the next line.
        unsafe {
            SetWindowLongPtrW(self.hwnd, GWLP_USERDATA, 0);
            let _ = DestroyWindow(self.hwnd);
        }
    }
}

/// The outside size of a window whose client area is `size`.
///
/// A plugin's size is the area it draws in; `CreateWindowExW` takes the size including the title
/// bar and borders. Passing the first as the second gives an interface cropped by exactly the
/// height of its own title bar, at the bottom, where the controls are.
fn frame_for(
    size: GuiSize,
    style: windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE,
) -> Option<(i32, i32)> {
    let mut rect = RECT {
        left: 0,
        top: 0,
        right: i32::try_from(size.width).ok()?,
        bottom: i32::try_from(size.height).ok()?,
    };
    // SAFETY: `rect` is a live local and the style is one this module made.
    unsafe { AdjustWindowRectEx(&mut rect, style, false, WINDOW_EX_STYLE(0)) }.ok()?;
    Some((rect.right - rect.left, rect.bottom - rect.top))
}

/// The style a window was made with, for measuring its frame again.
fn style_of(hwnd: HWND) -> windows::Win32::UI::WindowsAndMessaging::WINDOW_STYLE {
    use windows::Win32::UI::WindowsAndMessaging::{GWL_STYLE, WINDOW_STYLE};
    // SAFETY: called only on a window this module owns and has not destroyed.
    WINDOW_STYLE(unsafe { GetWindowLongPtrW(hwnd, GWL_STYLE) } as u32)
}

/// The `HWND` inside a raw handle, if it is a Windows one.
fn hwnd_of(handle: RawWindowHandle) -> Option<HWND> {
    match handle {
        RawWindowHandle::Win32(handle) => Some(HWND(handle.hwnd.get() as *mut std::ffi::c_void)),
        _ => None,
    }
}

/// Registers the window class, once per process.
fn register_class() {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| {
        // SAFETY: called once, with a class whose every field is filled in below.
        unsafe {
            let class = WNDCLASSEXW {
                cbSize: size_of::<WNDCLASSEXW>() as u32,
                style: CS_HREDRAW | CS_VREDRAW,
                lpfnWndProc: Some(window_proc),
                hInstance: GetModuleHandleW(None).unwrap_or_default().into(),
                hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
                // Painted once, behind a plugin that covers it. Black rather than the window
                // colour because a light flash before the interface arrives is the one that gets
                // noticed.
                hbrBackground: HBRUSH(GetStockObject(BLACK_BRUSH).0),
                lpszClassName: CLASS,
                ..Default::default()
            };
            RegisterClassExW(&class);
        }
    });
}

/// Answers the window's messages, of which exactly one matters.
///
/// # Safety
///
/// Called by Windows with a window of this class. The pointer read out of it is one `open` put
/// there and `Drop` clears before the window goes.
unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    if message == WM_CLOSE {
        // Deliberately *not* `DestroyWindow`, which is what the default handler would do. The
        // plugin's own window is a child of this one and would be destroyed with it, behind the
        // back of the plugin that is still holding it. The flag is read on the next tick and the
        // two are torn down in the right order.
        // SAFETY: the pointer is either null or an `Arc` kept alive by the `HostWindow` that owns
        // this window.
        let flag = unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } as *const AtomicBool;
        if !flag.is_null() {
            // SAFETY: as above.
            unsafe { (*flag).store(true, Ordering::Release) };
        }
        return LRESULT(0);
    }
    // SAFETY: the default handler is always safe to call with the message it was given.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
