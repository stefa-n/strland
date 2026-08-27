//! Desktop integration — hosts wallpaper behind icons via WorkerW.

use windows::Win32::Foundation::{HWND, LPARAM, WPARAM, BOOL};
use windows::Win32::Graphics::Dwm::{DwmGetWindowAttribute, DWMWA_CLOAKED};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayMonitors, GetMonitorInfoW, HDC, HMONITOR, MONITORINFO, MonitorFromWindow,
    MONITOR_DEFAULTTONEAREST,
};
use windows::Win32::UI::WindowsAndMessaging::{
    EnumWindows, FindWindowExW, FindWindowW, GetClassNameW, GetDesktopWindow,
    GetSystemMetrics, GetWindowLongPtrW, GetWindowRect, GWL_EXSTYLE,
    IsIconic, IsWindowVisible, SendMessageW, GetWindowPlacement, SM_CXVIRTUALSCREEN,
    SM_CYVIRTUALSCREEN, SW_SHOWMAXIMIZED, WINDOWPLACEMENT, WS_EX_TOOLWINDOW,
};

const WM_SPAWN_WORKERW: u32 = 0x052C;
const DEFVIEW_CLASS: &str = "SHELLDLL_DefView";
const WORKERW_CLASS: &str = "WorkerW";

#[derive(Debug, Clone, Copy, Default)]
pub struct Monitor {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    pub height: i32,
}

/// Find desktop host (WorkerW or desktop window).
pub fn find_host() -> Option<HWND> {
    unsafe {
        if let Some(progman) = find_window_by_class("Progman") {
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

/// True if any app covers the desktop.
pub fn any_window_covers_desktop() -> bool {
    let mut found = false;
    unsafe {
        let _ = EnumWindows(
            Some(enum_maximized_proc),
            LPARAM(&mut found as *mut bool as isize),
        );
    }
    found
}

const SHELL_WINDOW_CLASSES: [&str; 4] =
    ["Progman", "WorkerW", "Shell_TrayWnd", "strpaper.WallpaperWindow"];

unsafe extern "system" fn enum_maximized_proc(hwnd: HWND, lparam: LPARAM) -> BOOL { unsafe {
    let found = lparam.0 as *mut bool;

    if !IsWindowVisible(hwnd).as_bool() || IsIconic(hwnd).as_bool() {
        return true.into();
    }

    let mut cloaked = 0u32;
    if DwmGetWindowAttribute(
        hwnd,
        DWMWA_CLOAKED,
        &mut cloaked as *mut u32 as *mut core::ffi::c_void,
        std::mem::size_of::<u32>() as u32,
    )
    .is_ok()
        && cloaked != 0
    {
        return true.into();
    }

    let exstyle = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
    if exstyle & WS_EX_TOOLWINDOW.0 as isize != 0 {
        return true.into();
    }

    let mut cls = [0u16; 64];
    let n = GetClassNameW(hwnd, &mut cls);
    let name = String::from_utf16_lossy(&cls[..(n.max(0) as usize).min(cls.len())]);
    if SHELL_WINDOW_CLASSES.iter().any(|c| name.eq_ignore_ascii_case(c)) {
        return true.into();
    }

    let mut placement = WINDOWPLACEMENT::default();
    placement.length = std::mem::size_of::<WINDOWPLACEMENT>() as u32;
    if GetWindowPlacement(hwnd, &mut placement).is_ok()
        && placement.showCmd == SW_SHOWMAXIMIZED.0 as u32
    {
        *found = true;
        return false.into();
    }

    let mut rect = windows::Win32::Foundation::RECT::default();
    if GetWindowRect(hwnd, &mut rect).is_ok() {
        let monitor = MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST);
        let mut info = MONITORINFO::default();
        info.cbSize = std::mem::size_of::<MONITORINFO>() as u32;
        if GetMonitorInfoW(monitor, &mut info).as_bool()
            && rect.left <= info.rcMonitor.left
            && rect.top <= info.rcMonitor.top
            && rect.right >= info.rcMonitor.right
            && rect.bottom >= info.rcMonitor.bottom
        {
            *found = true;
            return false.into();
        }
    }
    true.into()
}}

pub fn monitors() -> Vec<Monitor> {
    unsafe { monitor_list() }
}

/// Bounding box of monitors.
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

/// Find WorkerW behind desktop icons.
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
    if find_defview_child(hwnd) {
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
