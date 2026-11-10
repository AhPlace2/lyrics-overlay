// Small Windows calls: the pointer, keys, the work area, window styles, one running copy.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT, RECT};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongPtrW, SetWindowLongPtrW, SetWindowPos, SystemParametersInfoW, GWL_EXSTYLE, HWND_TOPMOST, SPI_GETWORKAREA,
    SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOSIZE, SWP_SHOWWINDOW, SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS, STYLESTRUCT, WM_STYLECHANGING, WS_EX_APPWINDOW, WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW,
    WS_EX_TRANSPARENT,
};

pub fn cursor_pos() -> (i32, i32) {
    let mut p = POINT::default();
    unsafe {
        let _ = GetCursorPos(&mut p);
    }
    (p.x, p.y)
}

pub fn is_down(vk: i32) -> bool {
    unsafe { (GetAsyncKeyState(vk) as u16 & 0x8000) != 0 }
}

pub fn alt_down() -> bool {
    is_down(0x12)
}

/// "ctrl", "shift" or "alt" when just that one is down, "up" when none is, "busy" when it's part of a shortcut.
pub fn modifier_alone() -> &'static str {
    let keys = [("ctrl", 0x11), ("shift", 0x10), ("alt", 0x12)];
    let down: Vec<&'static str> = keys.iter().filter(|(_, vk)| is_down(*vk)).map(|(k, _)| *k).collect();
    if down.is_empty() {
        return "up";
    }
    if down.len() > 1 {
        return "busy"; // Ctrl+Alt for the overlay's own shortcuts, AltGr
    }
    // Any other key down means a shortcut like Ctrl+C. 0x01-0x06 are the mouse buttons, which may be down.
    for vk in 0x08..=0xfe {
        if (0x10..=0x12).contains(&vk) || (0xa0..=0xa5).contains(&vk) {
            continue;
        }
        if is_down(vk) {
            return "busy";
        }
    }
    down[0]
}

/// The primary screen minus the taskbar, in physical pixels: (x, y, width, height).
pub fn work_area() -> (i32, i32, i32, i32) {
    let mut r = RECT::default();
    unsafe {
        let _ = SystemParametersInfoW(SPI_GETWORKAREA, 0, Some(&mut r as *mut _ as *mut _), SYSTEM_PARAMETERS_INFO_UPDATE_FLAGS(0));
    }
    (r.left, r.top, r.right - r.left, r.bottom - r.top)
}

/// A tool window that never takes the focus. Chrome pauses video when it thinks a window covers it, but it ignores
/// tool windows; and the overlay must never take the keyboard from the app you're using.
/// It is also a layered window (its picture comes from our own bitmap) that lets clicks through to begin with.
/// winit rewrites the extended style from its own flags now and then, which would drop all of this, so a subclass
/// keeps our bits on every style change.
pub fn make_overlay_window(hwnd: isize) {
    OVERLAY.store(hwnd, Ordering::Relaxed);
    let hwnd = HWND(hwnd as *mut _);
    unsafe {
        let _ = SetWindowSubclass(hwnd, Some(keep_style), 1, 0);
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex);
        let _ = SetWindowPos(hwnd, Some(HWND_TOPMOST), 0, 0, 0, 0, SWP_NOMOVE | SWP_NOSIZE | SWP_NOACTIVATE);
    }
}

static PASS: AtomicBool = AtomicBool::new(true);
static ACTIVATABLE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn keep_style(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_STYLECHANGING && wp.0 as i32 == GWL_EXSTYLE.0 {
        let ss = &mut *(lp.0 as *mut STYLESTRUCT);
        let mut n = ss.styleNew | WS_EX_LAYERED.0 | WS_EX_TOOLWINDOW.0;
        n &= !WS_EX_APPWINDOW.0;
        n = if PASS.load(Ordering::Relaxed) { n | WS_EX_TRANSPARENT.0 } else { n & !WS_EX_TRANSPARENT.0 };
        n = if ACTIVATABLE.load(Ordering::Relaxed) { n & !WS_EX_NOACTIVATE.0 } else { n | WS_EX_NOACTIVATE.0 };
        ss.styleNew = n;
    }
    DefSubclassProc(hwnd, msg, wp, lp)
}

/// Sets the extended style again, which the subclass fills in from PASS and ACTIVATABLE.
fn restyle(hwnd: HWND) {
    unsafe {
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, ex ^ WS_EX_TRANSPARENT.0 as isize);
    }
}

pub static OVERLAY: AtomicIsize = AtomicIsize::new(0);

/// Lets clicks through the overlay to the apps below, or catches them.
pub fn set_passthrough(on: bool) {
    PASS.store(on, Ordering::Relaxed);
    let h = OVERLAY.load(Ordering::Relaxed);
    if h != 0 {
        restyle(HWND(h as *mut _));
    }
}

/// Puts a window there (physical pixels), on top, without taking the focus.
pub fn place(hwnd: isize, x: i32, y: i32, w: i32, h: i32) {
    unsafe {
        let _ = SetWindowPos(HWND(hwnd as *mut _), Some(HWND_TOPMOST), x, y, w, h, SWP_NOACTIVATE | SWP_SHOWWINDOW);
    }
}

/// While the settings panel is open the overlay takes the keyboard (Esc, Ctrl+Tab, the colour code); otherwise it
/// never does.
pub fn set_activatable(hwnd: isize, on: bool) {
    ACTIVATABLE.store(on, Ordering::Relaxed);
    let hwnd = HWND(hwnd as *mut _);
    restyle(hwnd);
    if on {
        unsafe {
            let _ = windows::Win32::UI::WindowsAndMessaging::SetForegroundWindow(hwnd);
        }
    }
}

pub fn set_app_id() {
    unsafe {
        let _ = windows::Win32::UI::Shell::SetCurrentProcessExplicitAppUserModelID(w!("Lyrics Overlay"));
    }
}

/// True when this is the only copy running. A second copy pokes the first (which shows a message) and quits.
pub fn single_instance(on_second: impl Fn() + Send + 'static) -> bool {
    unsafe {
        let _mutex = CreateMutexW(None, true, w!("Local\\LyricsOverlayNative"));
        if GetLastError() == ERROR_ALREADY_EXISTS {
            if let Ok(ev) = OpenEventW(EVENT_MODIFY_STATE, false, w!("Local\\LyricsOverlayNativeWake")) {
                let _ = SetEvent(ev);
                let _ = CloseHandle(ev);
            }
            return false;
        }
        // Kept for the life of the app: the mutex marks it as running.
        std::mem::forget(_mutex);
        if let Ok(ev) = CreateEventW(None, false, false, w!("Local\\LyricsOverlayNativeWake")) {
            let ev = ev.0 as isize;
            std::thread::spawn(move || loop {
                if WaitForSingleObject(HANDLE(ev as *mut _), INFINITE).0 == 0 {
                    on_second();
                }
            });
        }
        true
    }
}
