//! Win32 desktop-integration layer.
//!
//! `strpaper` paints its wallpaper into a child window that is reparented into
//! the desktop's **background** WorkerW — the window behind the desktop icons.
//! Because the wallpaper is a child of the desktop window, it is always drawn
//! underneath every application window, and no visible top-level window of our
//! own is ever created (so there is no taskbar button and no Alt+Tab entry).
//!
//! When Explorer is absent (a custom shell) it falls back to the desktop
//! window itself.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM, BOOL};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetDesktopWindow, GetSystemMetrics, SendMessageW,
    SM_CXVIRTUALSCREEN, SM_CYVIRTUALSCREEN,
};

/// The magic message that makes Explorer create/refresh the desktop WorkerW.
const WM_SPAWN_WORKERW: u32 = 0x052C;

/// The child window class that carries the desktop icons (`SHELLDLL_DefView`).
const DEFVIEW_CLASS: &str = "SHELLDLL_DefView";

/// The WorkerW window class.
const WORKERW_CLASS: &str = "WorkerW";

/// A single display monitor's rectangle in virtual desktop coordinates.
#[derive(Debug, Clone, Copy, Default)]
pub struct Monitor {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Find the desktop window that our wallpaper child window should be a child
/// of. This is the background WorkerW behind the icons when Explorer is
/// present, otherwise the desktop window.
pub fn find_host() -> Option<HWND> {
    unsafe {
        if let Some(progman) = find_window_by_class("Progman") {
            // Ask Explorer to (re)create a WorkerW behind the icons.
            let _ = SendMessageW(progman, WM_SPAWN_WORKERW, WPARAM(0x0D), LPARAM(0));
            if let Some(worker) = worker_behind_defview() {
                return Some(worker);
            }
            return Some(progman);
        }

        let sys = GetDesktopWindow();
        if !sys.is_invalid() {
            return Some(sys);
        }

        None
    }
}

/// Enumerate the monitors of the virtual desktop.
pub fn monitors() -> Vec<Monitor> {
    unsafe { monitor_list() }
}

/// Compute the bounding box of `monitors` as `(min_x, min_y, width, height)`.
/// This is the coordinate space our wallpaper window is painted into.
pub fn bounds(monitors: &[Monitor]) -> (i32, i32, i32, i32) {
    let mut min_x = i32::MAX;
    let mut min_y = i32::MAX;
    let mut max_x = i32::MIN;
    let mut max_y = i32::MIN;
    for m in monitors {
        min_x = min_x.min(m.left);
        min_y = min_y.min(m.top);
        max_x = max_x.max(m.left + m.width);
        max_y = max_y.max(m.top + m.height);
    }
    if min_x == i32::MAX {
        min_x = 0;
        min_y = 0;
        max_x = 1;
        max_y = 1;
    }
    (min_x, min_y, max_x - min_x, max_y - min_y)
}

unsafe fn find_window_by_class(class: &str) -> Option<HWND> { unsafe {
    let mut wclass: Vec<u16> = class.encode_utf16().chain(std::iter::once(0)).collect();
    let hwnd = FindWindowW(windows::core::PCWSTR(wclass.as_mut_ptr()), None).ok()?;
    if hwnd.is_invalid() {
        None
    } else {
        Some(hwnd)
    }
}}

/// Find the WorkerW that sits directly **behind** the desktop icon host. This
/// is the background surface the wallpaper must be painted onto.
unsafe fn worker_behind_defview() -> Option<HWND> { unsafe {
    let mut result: Option<HWND> = None;
    let _ = EnumWindows(
        Some(enum_windows_proc),
        LPARAM(&mut result as *mut Option<HWND> as isize),
    );
    result
}}

unsafe extern "system" fn enum_windows_proc(hwnd: HWND, lparam: LPARAM) -> BOOL { unsafe {
    let ptr = lparam.0 as *mut Option<HWND>;
    // `hwnd` hosts the desktop icons?
    if find_defview_child(hwnd) {
        // Grab the WorkerW sibling that is right below the icon host.
        let mut wclass: Vec<u16> = WORKERW_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
        if let Ok(worker) = FindWindowExW(
            HWND::default(),
            hwnd,
            windows::core::PCWSTR(wclass.as_mut_ptr()),
            windows::core::PCWSTR::null(),
        ) {
            if !worker.is_invalid() {
                *ptr = Some(worker);
                return false.into();
            }
        }
    }
    true.into()
}}

unsafe fn find_defview_child(hwnd: HWND) -> bool { unsafe {
    let mut defview: Vec<u16> = DEFVIEW_CLASS.encode_utf16().chain(std::iter::once(0)).collect();
    match FindWindowExW(
        hwnd,
        HWND::default(),
        windows::core::PCWSTR(defview.as_mut_ptr()),
        windows::core::PCWSTR::null(),
    ) {
        Ok(child) => !child.is_invalid(),
        Err(_) => false,
    }
}}

/// Build the list of monitors using Windows' virtual-screen coordinate space.
unsafe fn monitor_list() -> Vec<Monitor> { unsafe {
    let mut monitors: Vec<Monitor> = Vec::new();
    let _ = EnumDisplayMonitors(
        None,
        None,
        Some(monitor_enum_proc),
        LPARAM(&mut monitors as *mut Vec<Monitor> as isize),
    );
    if monitors.is_empty() {
        let cx = GetSystemMetrics(SM_CXVIRTUALSCREEN);
        let cy = GetSystemMetrics(SM_CYVIRTUALSCREEN);
        monitors.push(Monitor {
            left: 0,
            top: 0,
            width: cx,
            height: cy,
        });
    }
    monitors
}}

unsafe extern "system" fn monitor_enum_proc(
    hmonitor: HMONITOR,
    _hdc: HDC,
    _rect: *mut windows::Win32::Foundation::RECT,
    lparam: LPARAM,
) -> BOOL { unsafe {
    let ptr = lparam.0 as *mut Vec<Monitor>;
    let mut info = MONITORINFO::default();
    info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
    if GetMonitorInfoW(hmonitor, &mut info).as_bool() {
        (*ptr).push(Monitor {
            left: info.rcMonitor.left,
            top: info.rcMonitor.top,
            width: info.rcMonitor.right - info.rcMonitor.left,
            height: info.rcMonitor.bottom - info.rcMonitor.top,
        });
    }
    true.into()
}}
