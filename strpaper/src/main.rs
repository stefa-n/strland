//! `strpaper` — a live wallpaper engine for the Strland shell.
//!
//! Wallpapers are read from a per-user storage directory:
//!
//! ```text
//! %USERPROFILE%\.strland\strpaper\
//! ```
//!
//! The directory is created at startup if it is missing. `strpaper` watches it
//! and hot-reloads whenever `wallpaper.{png,jpg,jpeg,bmp,gif,mp4,webm}` is
//! added, replaced or removed.
//!
//! Rendering is performed by a child window reparented into the desktop's
//! background WorkerW (behind the desktop icons), so it is always drawn
//! underneath every application window. The application creates no visible
//! top-level window: no taskbar button, no Alt+Tab entry, no admin rights.

#![windows_subsystem = "windows"]

mod desktop;
mod logger;
mod render;
mod storage;
mod video;
mod watch;

use std::path::PathBuf;
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use desktop::Monitor;
use logger::log;
use render::{Wallpaper, decode_animated, decode_still, frame_at, needs_ticks};
use storage::Candidate;
use watch::WallpaperWatcher;

use windows::core::w;
use windows::Win32::Foundation::{BOOL, HWND, LPARAM, LRESULT, WPARAM};
use windows::Win32::Graphics::Gdi::{BeginPaint, EndPaint, InvalidateRect, PAINTSTRUCT};
use windows::Win32::System::Console::{
    CTRL_BREAK_EVENT, CTRL_C_EVENT, CTRL_CLOSE_EVENT, SetConsoleCtrlHandler,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, SetProcessDpiAwarenessContext,
};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW,
    GetWindowLongPtrW, HWND_BOTTOM, IDC_ARROW, KillTimer, LoadCursorW, PostMessageW,
    PostQuitMessage, RegisterClassW, SetParent, SetTimer, SetWindowLongPtrW, SetWindowPos,
    ShowWindow, TranslateMessage, GWLP_USERDATA, MSG, SWP_NOACTIVATE, SWP_SHOWWINDOW, SW_HIDE,
    SW_SHOWNOACTIVATE, WINDOW_EX_STYLE, WINDOW_STYLE, WM_DESTROY, WM_DISPLAYCHANGE,
    WM_ERASEBKGND, WM_PAINT, WM_TIMER, WNDCLASSW,
};

/// Timer identifier.
const TIMER_MAIN: usize = 1;

/// Custom message used to request a graceful shutdown (from Ctrl+C/Run). WM_APP
/// (0x8000) + 1 so it never collides with system messages.
const WM_APP_QUIT: u32 = 0x8000 + 1;

/// Repaint cadence (ms) for animated/video wallpapers (~24 - 30 fps).
const MOTION_MS: u32 = 33;
/// Poll cadence (ms) used to detect filesystem changes for still wallpapers.
const POLL_MS: u32 = 250;
/// Brief settle time after a filesystem change, to avoid reading a partially
/// written file.
const SETTLE_MS: u64 = 200;

/// If a video has not produced its first frame within this many seconds it is
/// treated as stalled and the desktop is revealed (instead of a grey frame).
const VIDEO_TIMEOUT_SECS: f64 = 5.0;

/// The wallpaper window handle, published for the console-ctrl handler.
static WALLPAPER_HWND: AtomicIsize = AtomicIsize::new(0);

fn main() {
    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    if let Err(e) = video::startup() {
        log(&format!("Media Foundation unavailable: {e}"));
    }

    let dir = storage::wallpaper_dir();
    logger::start(&storage::display_dir(&dir));

    let watcher = match WallpaperWatcher::new(&dir) {
        Ok(w) => Watcher::Live(w),
        Err(e) => {
            log(&format!("file watching disabled: {e}"));
            Watcher::Poll
        }
    };

    let mut app = App {
        dir,
        watcher,
        host: None,
        hwnd: HWND::default(),
        wallpaper: None,
        frame_start: Instant::now(),
        motion: false,
        timer_ms: POLL_MS,
        monitors: Vec::new(),
        origin: (0, 0),
        painted_once: false,
        painted_version: 0,
    };

    run_message_loop(&mut app);

    // Drop the app (joins the video decode thread) before shutting down MF.
    drop(app);
    video::shutdown();
}

/// App state shared with the wallpaper window procedure.
struct App {
    dir: PathBuf,
    watcher: Watcher,
    /// The desktop window our child window is reparented into.
    host: Option<HWND>,
    /// Our wallpaper child window.
    hwnd: HWND,
    wallpaper: Option<Wallpaper>,
    frame_start: Instant,
    motion: bool,
    timer_ms: u32,
    monitors: Vec<Monitor>,
    origin: (i32, i32),
    painted_once: bool,
    /// Version of the last video frame we painted (skip redundant repaints).
    painted_version: u64,
}

enum Watcher {
    Live(WallpaperWatcher),
    Poll,
}

impl App {
    /// Refresh the monitor list / bounding-box origin (used at startup and on
    /// display change), then re-size the wallpaper window to cover it.
    fn refresh_monitors(&mut self) {
        self.monitors = desktop::monitors();
        let (min_x, min_y, w, h) = desktop::bounds(&self.monitors);
        self.origin = (min_x, min_y);

        if self.host.is_some() {
            if !self.hwnd.is_invalid() {
                // Position relative to the parent's client origin (the desktop
                // origin == min_x,min_y), covering the whole virtual desktop.
                let _ = unsafe {
                    SetWindowPos(
                        self.hwnd,
                        HWND_BOTTOM,
                        0,
                        0,
                        w,
                        h,
                        SWP_NOACTIVATE | SWP_SHOWWINDOW,
                    )
                };
            }
        }
    }

    /// Re-resolve the wallpaper file from the storage directory and load it.
    fn reload(&mut self) {
        let mut candidates = storage::list_candidates(&self.dir);
        match storage::choose_primary(&mut candidates) {
            Some(candidate) => {
                let target = self.target_size();
                self.frame_start = Instant::now();
                self.wallpaper = load_wallpaper(&candidate, target);
                self.painted_version = 0;
                self.motion = self
                    .wallpaper
                    .as_ref()
                    .map(|w| needs_ticks(w))
                    .unwrap_or(false);
                self.timer_ms = if self.motion { MOTION_MS } else { POLL_MS };
                if self.wallpaper.is_some() {
                    self.show_window();
                }
            }
            None => {
                // No wallpaper present: hide the rendered wallpaper and keep
                // running (a new wallpaper is picked up on the next change).
                self.wallpaper = None;
                self.motion = false;
                self.timer_ms = POLL_MS;
                self.hide_window();
            }
        }
        // Repaint once with the new wallpaper (so a just-loaded image shows).
        self.invalidate();
    }

    /// The size videos should be decoded into — the desktop's bounding box, so
    /// we never convert or blit more pixels than are actually shown.
    fn target_size(&self) -> Option<(u32, u32)> {
        if self.monitors.is_empty() {
            return None;
        }
        let (_x, _y, w, h) = desktop::bounds(&self.monitors);
        if w > 0 && h > 0 {
            Some((w as u32, h as u32))
        } else {
            None
        }
    }

    fn show_window(&self) {
        if self.host.is_some() && !self.hwnd.is_invalid() {
            let _ = unsafe { ShowWindow(self.hwnd, SW_SHOWNOACTIVATE) };
        }
    }

    fn hide_window(&self) {
        if !self.hwnd.is_invalid() {
            let _ = unsafe { ShowWindow(self.hwnd, SW_HIDE) };
        }
    }

    /// Re-locate the desktop window after a display configuration change.
    fn on_display_change(&mut self) {
        self.host = desktop::find_host();
        if let Some(host) = self.host {
            if !self.hwnd.is_invalid() {
                let _ = unsafe { SetParent(self.hwnd, host) };
            }
        }
        self.refresh_monitors();
        self.invalidate();
    }

    /// One timer tick: pick up pending file changes and advance motion.
    fn tick(&mut self) {
        if self.watcher_is_dirty() {
            std::thread::sleep(Duration::from_millis(SETTLE_MS));
            self.wallpaper = None;
            self.reload();
        }

        // If the media could not be decoded (or stalled with no frames), treat
        // it as absent and reveal the desktop instead of freezing on grey.
        let (video_failed, reason, stalled) = match self.wallpaper.as_ref() {
            Some(Wallpaper::Video(v)) => (
                v.is_failed(),
                v.failure_reason(),
                !v.has_yielded() && v.active_secs() > VIDEO_TIMEOUT_SECS,
            ),
            _ => (false, None, false),
        };
        if video_failed || stalled {
            if stalled {
                log(&format!(
                    "video: no frames within {VIDEO_TIMEOUT_SECS}s (treating as failed)"
                ));
            }
            if let Some(r) = reason {
                log(&format!("video failed: {r}"));
            }
            self.wallpaper = None;
            self.motion = false;
            self.timer_ms = POLL_MS;
            self.hide_window();
        }

        // Animation / video: redraw only when there is something new to show.
        if self.motion {
            match self.wallpaper.as_ref() {
                Some(Wallpaper::Video(v)) => {
                    let version = v.version();
                    if version != self.painted_version {
                        self.painted_version = version;
                        self.invalidate();
                    }
                }
                _ => self.invalidate(), // GIF: always advance
            }
        }
    }

    fn invalidate(&self) {
        if !self.hwnd.is_invalid() {
            let _ = unsafe { InvalidateRect(self.hwnd, None, false) };
        }
    }

    /// Paint the current wallpaper frame into the window's client DC.
    fn paint(&mut self) {
        if self.hwnd.is_invalid() {
            return;
        }
        let Some(wallpaper) = self.wallpaper.as_mut() else {
            return; // window is hidden; nothing to paint
        };
        unsafe {
            let mut ps = PAINTSTRUCT::default();
            let hdc = BeginPaint(self.hwnd, &mut ps);
            if let Some(raster) = frame_at(wallpaper, self.frame_start.elapsed()) {
                render::paint_frame(hdc, &self.monitors, self.origin, raster.as_ref());
                if !self.painted_once {
                    self.painted_once = true;
                    log(&format!(
                        "painted frame {}x{} over {} monitor(s)",
                        raster.width,
                        raster.height,
                        self.monitors.len()
                    ));
                }
            } else {
                render::paint_clear(hdc, &self.monitors, self.origin);
            }
            let _ = EndPaint(self.hwnd, &ps);
        }
    }

    fn watcher_is_dirty(&self) -> bool {
        match &self.watcher {
            Watcher::Live(w) => w.is_dirty(),
            Watcher::Poll => false,
        }
    }
}

/// Run the hidden message loop for the foreground lifetime of the process.
fn run_message_loop(app: &mut App) {
    unsafe {
        let hinstance = GetModuleHandleW(None).expect("GetModuleHandleW");
        let class_name = w!("strpaper.WallpaperWindow");

        let wc = WNDCLASSW {
            lpfnWndProc: Some(wnd_proc),
            hInstance: windows::Win32::Foundation::HINSTANCE(hinstance.0),
            lpszClassName: class_name,
            // Without this the desktop shows a busy cursor when hovered.
            hCursor: LoadCursorW(None, IDC_ARROW).unwrap_or_default(),
            ..Default::default()
        };
        if RegisterClassW(&wc) == 0 {
            eprintln!("strpaper: RegisterClassW failed");
            return;
        }

        // Locate the desktop background (the WorkerW behind the icons, or the
        // desktop window) so we can attach our child window to it.
        let host = desktop::find_host();
        app.host = host;

        // Size/position from the monitors now; refine on display change.
        app.monitors = desktop::monitors();
        let (min_x, min_y, mon_w, mon_h) = desktop::bounds(&app.monitors);
        app.origin = (min_x, min_y);

        // Create the wallpaper window. When a desktop host exists, create it
        // as a direct CHILD of that window so it is always painted underneath
        // every application window. It starts hidden and is shown once it is
        // placed at the bottom of the desktop (behind the icons).
        let (style, parent) = match host {
            Some(h) => (WINDOW_STYLE(0x4000_0000 | 0x0200_0000), h), // CHILD | CLIPCHILDREN
            None => (WINDOW_STYLE(0x8000_0000 | 0x0200_0000), HWND::default()), // POPUP | CLIPCHILDREN
        };
        let ex_style = WINDOW_EX_STYLE(0x0000_0080 | 0x0800_0000 | 0x0000_0020); // TOOLWINDOW | NOACTIVATE | TRANSPARENT

        let hwnd = CreateWindowExW(
            ex_style,
            class_name,
            w!("strpaper"),
            style,
            0,
            0,
            mon_w.max(1),
            mon_h.max(1),
            parent,
            None,
            windows::Win32::Foundation::HINSTANCE(hinstance.0),
            None,
        )
        .expect("CreateWindowExW");

        app.hwnd = hwnd;
        WALLPAPER_HWND.store(hwnd.0 as isize, Ordering::SeqCst);
        SetWindowLongPtrW(hwnd, GWLP_USERDATA, app as *mut App as isize);

        app.refresh_monitors(); // show behind the icons now that we know the host
        app.reload();

        // Make Ctrl+C / Ctrl+Break / console close graceful.
        let _ = SetConsoleCtrlHandler(Some(console_ctrl_handler), true);

        let _ = SetTimer(hwnd, TIMER_MAIN, app.timer_ms, None);

        let mut msg = MSG::default();
        loop {
            if GetMessageW(&mut msg, None, 0, 0).as_bool() {
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            } else {
                break;
            }
        }

        let _ = SetConsoleCtrlHandler(None, false);
        let _ = KillTimer(hwnd, TIMER_MAIN);
    }
}

/// Handle OS ctrl events (Ctrl+C, Ctrl+Break, close) by waking the message
/// loop to shut down gracefully.
unsafe extern "system" fn console_ctrl_handler(ctrl: u32) -> BOOL { unsafe {
    match ctrl {
        CTRL_C_EVENT | CTRL_BREAK_EVENT | CTRL_CLOSE_EVENT => {
            let value = WALLPAPER_HWND.load(Ordering::SeqCst) as *mut core::ffi::c_void;
            let hwnd = HWND(value);
            if !hwnd.is_invalid() {
                let _ = PostMessageW(hwnd, WM_APP_QUIT, WPARAM(0), LPARAM(0));
            }
            true.into()
        }
        _ => false.into(),
    }
}}

/// Window procedure for our wallpaper window.
unsafe extern "system" fn wnd_proc(
    hwnd: HWND,
    msg: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT { unsafe {
    match msg {
        WM_PAINT => {
            let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if let Some(app) = app.as_mut() {
                app.paint();
            }
            LRESULT(0)
        }
        WM_ERASEBKGND => LRESULT(1), // we fully paint; avoid background flicker
        WM_TIMER => {
            let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            let interval = if let Some(app) = app.as_mut() {
                app.tick();
                app.timer_ms
            } else {
                POLL_MS
            };
            // Re-arm the timer if the desired cadence changed (static <-> motion).
            let _ = SetTimer(hwnd, TIMER_MAIN, interval, None);
            LRESULT(0)
        }
        WM_DISPLAYCHANGE => {
            let app = GetWindowLongPtrW(hwnd, GWLP_USERDATA) as *mut App;
            if let Some(app) = app.as_mut() {
                app.on_display_change();
            }
            LRESULT(0)
        }
        WM_APP_QUIT => {
            let _ = DestroyWindow(hwnd);
            LRESULT(0)
        }
        WM_DESTROY => {
            PostQuitMessage(0);
            LRESULT(0)
        }
        _ => DefWindowProcW(hwnd, msg, wparam, lparam),
    }
}}

/// Load a wallpaper file into a [`Wallpaper`], returning `None` on failure.
fn load_wallpaper(candidate: &Candidate, target: Option<(u32, u32)>) -> Option<Wallpaper> {
    let path = &candidate.path;
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();

    let result = match ext.as_str() {
        "gif" => decode_animated(path)
            .map(Wallpaper::Animated)
            .map_err(|e| format!("gif: {e}")),
        "mp4" | "webm" => video::VideoPlayer::open(path, target)
            .map(Wallpaper::Video)
            .map_err(|e| format!("video: {e}")),
        "png" | "jpg" | "jpeg" | "bmp" => decode_still(path)
            .map(|r| Wallpaper::Still(Arc::new(r)))
            .map_err(|e| format!("image: {e}")),
        other => Err(format!("unsupported extension: {other}")),
    };

    match result {
        Ok(w) => {
            log(&format!("loaded {}", path.display()));
            Some(w)
        }
        Err(e) => {
            log(&format!("failed to load {} ({e})", path.display()));
            None
        }
    }
}
