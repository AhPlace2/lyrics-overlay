// Small Windows calls: the pointer, keys, the work area, window styles, one running copy.

use std::sync::atomic::{AtomicBool, AtomicIsize, Ordering};
use windows::core::w;
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::UI::Shell::{DefSubclassProc, SetWindowSubclass};
use windows::Win32::Foundation::{CloseHandle, GetLastError, ERROR_ALREADY_EXISTS, HANDLE, HWND, POINT, RECT};
use windows::Win32::System::Threading::{CreateEventW, CreateMutexW, OpenEventW, SetEvent, WaitForSingleObject, EVENT_MODIFY_STATE, INFINITE};
use windows::Win32::UI::Input::KeyboardAndMouse::GetAsyncKeyState;
use windows::Win32::UI::WindowsAndMessaging::{
    GetCursorPos, GetWindowLongPtrW, SetWindowDisplayAffinity, SetWindowLongPtrW, SetWindowPos, SystemParametersInfoW, GWL_EXSTYLE, HWND_TOPMOST, SPI_GETWORKAREA,
    WDA_EXCLUDEFROMCAPTURE, WDA_NONE,
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
/// The GPU build: no layered bitmap of our own, so the window is layered only while clicks pass through
/// (Windows needs WS_EX_LAYERED with WS_EX_TRANSPARENT for that).
pub static GPU: AtomicBool = AtomicBool::new(false);
static ACTIVATABLE: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn keep_style(hwnd: HWND, msg: u32, wp: WPARAM, lp: LPARAM, _id: usize, _data: usize) -> LRESULT {
    if msg == WM_STYLECHANGING && wp.0 as i32 == GWL_EXSTYLE.0 {
        let ss = &mut *(lp.0 as *mut STYLESTRUCT);
        let mut n = ss.styleNew | WS_EX_TOOLWINDOW.0;
        let pass = PASS.load(Ordering::Relaxed);
        n = if !GPU.load(Ordering::Relaxed) || pass { n | WS_EX_LAYERED.0 } else { n & !WS_EX_LAYERED.0 };
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

/// Keeps the overlay out of screenshots, recordings and screen sharing (Zoom, Teams, Discord, OBS) while it stays on
/// your own screen. Needs Windows 10 2004 or later; older versions just keep showing it.
pub fn set_capture_hidden(on: bool) {
    let h = OVERLAY.load(Ordering::Relaxed);
    if h == 0 {
        return;
    }
    unsafe {
        if let Err(e) = SetWindowDisplayAffinity(HWND(h as *mut _), if on { WDA_EXCLUDEFROMCAPTURE } else { WDA_NONE }) {
            eprintln!("Could not change screen capture: {e}");
        }
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
/// With `wait` (started by the updater) it instead waits up to 10 s for the other copy to quit.
pub fn single_instance(on_second: impl Fn() + Send + 'static, wait: bool) -> bool {
    unsafe {
        let mut _mutex = CreateMutexW(None, true, w!("Local\\LyricsOverlayNative"));
        if wait {
            for _ in 0..50 {
                if GetLastError() != ERROR_ALREADY_EXISTS {
                    break;
                }
                if let Ok(m) = &_mutex {
                    let _ = CloseHandle(*m);
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
                _mutex = CreateMutexW(None, true, w!("Local\\LyricsOverlayNative"));
            }
        }
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

/// Windows' Open dialog for one font file. Blocks until it closes, so call it off the UI thread.
pub fn pick_font_file() -> Option<std::path::PathBuf> {
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Controls::Dialogs::{GetOpenFileNameW, OFN_EXPLORER, OFN_FILEMUSTEXIST, OFN_NOCHANGEDIR, OFN_PATHMUSTEXIST, OPENFILENAMEW};
    use windows::core::{PCWSTR, PWSTR};
    let mut file = vec![0u16; 1024];
    let filter: Vec<u16> = "Fonts (*.ttf, *.otf)\0*.ttf;*.otf;*.ttc;*.otc\0\0".encode_utf16().collect();
    let mut ofn = OPENFILENAMEW {
        lStructSize: std::mem::size_of::<OPENFILENAMEW>() as u32,
        lpstrFilter: PCWSTR(filter.as_ptr()),
        lpstrFile: PWSTR(file.as_mut_ptr()),
        nMaxFile: file.len() as u32,
        lpstrTitle: w!("Add a font"),
        Flags: OFN_EXPLORER | OFN_FILEMUSTEXIST | OFN_PATHMUSTEXIST | OFN_NOCHANGEDIR,
        ..Default::default()
    };
    unsafe {
        let com = CoInitializeEx(None, COINIT_APARTMENTTHREADED).is_ok();
        let ok = GetOpenFileNameW(&mut ofn).as_bool();
        if com {
            CoUninitialize();
        }
        if !ok {
            return None;
        }
    }
    let len = file.iter().position(|c| *c == 0).unwrap_or(file.len());
    use std::os::windows::ffi::OsStringExt;
    Some(std::ffi::OsString::from_wide(&file[..len]).into())
}

/// A monitor: its device name ("\.\DISPLAY2"), work area (minus the taskbar) in physical pixels as
/// (x, y, width, height), and whether it is the main one.
#[derive(Clone, Debug)]
pub struct Monitor {
    pub id: String,
    pub area: (i32, i32, i32, i32),
    pub primary: bool,
}

/// Every monitor, the main one first, then left to right.
pub fn monitors() -> Vec<Monitor> {
    use windows::Win32::Graphics::Gdi::{EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW};
    unsafe extern "system" fn each(m: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> windows::core::BOOL {
        let list = unsafe { &mut *(list.0 as *mut Vec<Monitor>) };
        let mut info = MONITORINFOEXW::default();
        info.monitorInfo.cbSize = std::mem::size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(m, &mut info as *mut MONITORINFOEXW as *mut MONITORINFO) }.as_bool() {
            let w = info.monitorInfo.rcWork;
            let len = info.szDevice.iter().position(|c| *c == 0).unwrap_or(info.szDevice.len());
            list.push(Monitor {
                id: String::from_utf16_lossy(&info.szDevice[..len]),
                area: (w.left, w.top, w.right - w.left, w.bottom - w.top),
                primary: info.monitorInfo.dwFlags & 1 != 0, // MONITORINFOF_PRIMARY
            });
        }
        true.into()
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe {
        let _ = EnumDisplayMonitors(None, None, Some(each), LPARAM(&mut list as *mut Vec<Monitor> as isize));
    }
    list.sort_by_key(|m| (!m.primary, m.area.0, m.area.1));
    list
}

/// The work area of the monitor the "screen" setting names, or of the main one when it is "main" or not plugged in.
pub fn work_area_on(screen: &str) -> (i32, i32, i32, i32) {
    let list = monitors();
    list.iter()
        .find(|m| m.id == screen)
        .or_else(|| list.iter().find(|m| m.primary))
        .map(|m| m.area)
        .unwrap_or_else(work_area)
}

/// Names for the monitors, as the settings show them: the main one, then where each other one sits from it
/// ("Left", "Right", "Above", "Below"), numbered when two share a side.
pub fn monitor_names(list: &[Monitor]) -> Vec<String> {
    let Some(main) = list.iter().find(|m| m.primary).or(list.first()) else { return Vec::new() };
    let centre = |m: &Monitor| (m.area.0 as f64 + m.area.2 as f64 / 2.0, m.area.1 as f64 + m.area.3 as f64 / 2.0);
    let (mx, my) = centre(main);
    let sides: Vec<&str> = list
        .iter()
        .map(|m| {
            if m.id == main.id {
                return "Main";
            }
            let (x, y) = centre(m);
            let (dx, dy) = (x - mx, y - my);
            if dx.abs() >= dy.abs() {
                if dx < 0.0 { "Left" } else { "Right" }
            } else if dy < 0.0 {
                "Above"
            } else {
                "Below"
            }
        })
        .collect();
    sides
        .iter()
        .enumerate()
        .map(|(i, side)| {
            let same = sides.iter().filter(|s| s == &side).count();
            if same > 1 {
                format!("{side} {}", sides[..=i].iter().filter(|s| s == &side).count())
            } else {
                side.to_string()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(id: &str, x: i32, y: i32, primary: bool) -> Monitor {
        Monitor { id: id.into(), area: (x, y, 1920, 1040), primary }
    }

    #[test]
    fn monitor_names_by_side() {
        let list = [m("A", 0, 0, true), m("B", -1920, 0, false), m("C", 1920, 0, false), m("D", 3840, 0, false), m("E", 0, -1080, false)];
        assert_eq!(monitor_names(&list), ["Main", "Left", "Right 1", "Right 2", "Above"]);
    }

    #[test]
    fn this_machine() {
        let list = monitors();
        eprintln!("{:?} {:?}", list, monitor_names(&list));
        assert!(list.iter().filter(|m| m.primary).count() == 1);
        assert_eq!(work_area_on("main"), list[0].area);
        assert_eq!(work_area_on("nope"), list[0].area);
    }
}
