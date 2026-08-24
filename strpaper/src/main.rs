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
mod gpu;
mod logger;
mod render;
mod storage;
mod video;
mod watch;
mod widgets;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicIsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use desktop::Monitor;
use logger::log;
use render::{Wallpaper, decode_animated, decode_still, fit_cover, frame_at, needs_ticks};
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
    // Shell apps live in COM; initialize the main apartment up front so MF
    // probes on this thread behave.
    unsafe {
        let _ = windows::Win32::System::Com::CoInitializeEx(
            None,
            windows::Win32::System::Com::COINIT_APARTMENTTHREADED,
        );
    }

    // Temporary bisect switches (also useful as escape hatches).
    let no_widgets = std::env::var("STRPAPER_NO_WIDGETS").is_ok();

    unsafe {
        let _ = SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    }

    if let Err(e) = video::startup() {
        log(&format!("Media Foundation unavailable: {e}"));
    }

    let dir = storage::wallpaper_dir();

    let watcher = match WallpaperWatcher::new(&dir) {
        Ok(w) => Watcher::Live(w),
        Err(e) => {
            log(&format!("file watching disabled: {e}"));
            Watcher::Poll
        }
    };

    // Separate watcher for the widgets directory (its own dirty flag).
    let wdir = widgets::widgets_dir(&dir);
    widgets::ensure_sample(&wdir);
    let widgets_watcher = if no_widgets {
        None
    } else {
        match WallpaperWatcher::new(&wdir) {
            Ok(w) => Some(w),
            Err(e) => {
                log(&format!("widget watching disabled: {e}"));
                None
            }
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
        painted_version: 0,
        video_path: None,
        render_paused: false,
        loaded_source: None,
        config_name: None,
        media_cache: None,
        transcoded_cache: None,
        transcode_rx: None,
        transcode_generation: 0,
        widget_host: None,
        widgets_watcher,
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
    /// Version of the last video frame we painted (skip redundant repaints).
    painted_version: u64,
    /// Path of the currently loaded video, for GPU->software fallback.
    video_path: Option<PathBuf>,
    /// Rendering is suspended (maximized/fullscreen app covering the desktop).
    render_paused: bool,
    /// Programmable widgets (scripts above wallpaper, below icons/apps).
    widget_host: Option<widgets::WidgetHost>,
    /// Watches the widgets directory.
    widgets_watcher: Option<watch::WallpaperWatcher>,
    /// What is currently loaded, so unchanged files are never re-read.
    loaded_source: Option<(PathBuf, Option<SystemTime>)>,
    /// Last-read wallpaper name from the config file.
    config_name: Option<String>,
    /// Most recent media file kept in RAM so switching wallpapers does not
    /// re-read from disk.
    media_cache: Option<(PathBuf, SystemTime, Arc<Vec<u8>>)>,
    /// Pre-scaled re-encodes kept in RAM, keyed by source + output height.
    transcoded_cache: Option<((PathBuf, Option<SystemTime>, u32), Arc<Vec<u8>>)>,
    /// Results from background pre-conversion jobs.
    transcode_rx: Option<std::sync::mpsc::Receiver<TranscodeResult>>,
    /// Bumped whenever a new job is spawned; stale results are dropped.
    transcode_generation: u64,
}

/// A finished (or failed) background pre-conversion.
enum TranscodeResult {
    Done(u64, Vec<u8>),
    Failed(u64, String),
}

impl TranscodeResult {
    fn generation(&self) -> u64 {
        match self {
            TranscodeResult::Done(g, _) | TranscodeResult::Failed(g, _) => *g,
        }
    }
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

    /// Re-resolve the wallpaper from the config file + storage directory and
    /// load it. Unchanged sources are skipped entirely (nothing is re-read).
    fn reload(&mut self) {
        // The optional `wallpaper = "..."` entry in the config file selects
        // which file in the wallpaper directory to show.
        self.config_name = storage::read_configured_name(&self.dir);
        let Some(path) = storage::resolve_wallpaper_file(&self.dir, self.config_name.as_deref())
        else {
            // Nothing to display: hide and keep running.
            self.wallpaper = None;
            self.loaded_source = None;
            self.motion = false;
            self.timer_ms = POLL_MS;
            self.hide_window();
            self.invalidate();
            return;
        };

        let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();

        // Already showing this exact file? Nothing to do — the wallpaper stays
        // in memory and is not re-read or re-decoded.
        if self.loaded_source.as_ref().is_some_and(|(p, m)| {
            *p == path && *m == mtime && self.wallpaper.is_some()
        }) {
            return;
        }

        let target = self.target_size();
        let ext_is_video = path
            .extension()
            .and_then(|e| e.to_str())
            .map(|e| {
                let e = e.to_ascii_lowercase();
                e == "mp4" || e == "webm"
            })
            .unwrap_or(false);

        if ext_is_video {
            // Keep the media bytes cached so switching wallpapers does not
            // re-read them from disk.
            let Some((data, _)) = self.media_data(&path) else {
                log(&format!("failed to read {}", path.display()));
                self.wallpaper = None;
                self.loaded_source = None;
                self.hide_window();
                self.invalidate();
                return;
            };
            self.video_path = Some(path.clone());

            // Optional pre-conversion: re-encode once to the configured height
            // (in memory) so playback never has to chew through 4K frames.
            let quality_height = if std::env::var("STRPAPER_NO_TRANSCODE").is_ok() {
                None
            } else {
                storage::read_configured_quality(&self.dir)
            };
            if let Some(qh) = quality_height {
                let (src_w, src_h) =
                    video::VideoPlayer::probe_size(&data).unwrap_or((0, 0));
                if src_w > 0 && src_h > 0 && qh < src_h {
                    let out_h = (qh & !1).max(2);
                    let out_w = (((src_w as u64 * out_h as u64) / src_h as u64) as u32 & !1).max(2);

                    // Already converted for this file/size? Play it directly.
                    if let Some((key, bytes)) = &self.transcoded_cache {
                        if key.0 == path && key.1 == mtime && key.2 == out_h {
                            let tgt = self.target_size();
                            return self.open_video(bytes.clone(), tgt);
                        }
                    }

                    // Otherwise run the conversion in the background and keep
                    // the current wallpaper visible until it is ready.
                    self.transcode_generation += 1;
                    let generation = self.transcode_generation;
                    let (tx, rx) = std::sync::mpsc::channel();
                    self.transcode_rx = Some(rx);
                    self.loaded_source = Some((path.clone(), mtime));
                    std::thread::spawn(move || {
                        let result = video::VideoPlayer::transcode(data, out_w, out_h);
                        let _ = tx.send(match result {
                            Ok(bytes) => TranscodeResult::Done(generation, bytes),
                            Err(e) => TranscodeResult::Failed(generation, e),
                        });
                    });
                    return;
                }
            }

            self.frame_start = Instant::now();
            self.painted_version = 0;
            self.wallpaper =
                load_video(&data, target).or_else(|| {
                    log(&format!("failed to load {}", path.display()));
                    None
                });
        } else {
            self.video_path = None;
            self.frame_start = Instant::now();
            self.painted_version = 0;
            // Fit to the desktop size so widget compositing has matching
            // dimensions (and blitting is 1:1).
            let fit = self.target_size();
            let ext = path
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| e.to_ascii_lowercase())
                .unwrap_or_default();
            self.wallpaper = if ext == "gif" {
                decode_animated(&path)
                    .map(|mut a| {
                        if let Some((tw, th)) = fit {
                            for f in &mut a.frames {
                                f.raster =
                                    Arc::new(fit_cover(&f.raster, tw as usize, th as usize, true));
                            }
                        }
                        Wallpaper::Animated(a)
                    })
                    .map_err(|e| {
                        log(&format!(
                            "failed to load {} ({e:?})",
                            path.display()
                        ));
                        e
                    })
                    .ok()
            } else {
                decode_still(&path)
                    .map(|r| {
                        let r = match fit {
                            Some((tw, th)) => fit_cover(&r, tw as usize, th as usize, false),
                            None => r,
                        };
                        Wallpaper::Still(Arc::new(r))
                    })
                    .map_err(|e| {
                        log(&format!("failed to load {} ({e:?})", path.display()));
                        e
                    })
                    .ok()
            };
        }

        // Log only when the active file actually changes (not on no-op
        // reloads) so the log stays useful and quiet.
        if self
            .loaded_source
            .as_ref()
            .is_none_or(|(p, _)| *p != path)
        {
            match self.config_name.as_deref() {
                Some(name) => log(&format!("wallpaper set to {name} (from config)")),
                None => log(&format!("wallpaper set to {}", path.display())),
            }
        }

        self.loaded_source = self
            .wallpaper
            .as_ref()
            .map(|_| (path.clone(), mtime));
        self.motion = self
            .wallpaper
            .as_ref()
            .map(|w| needs_ticks(w))
            .unwrap_or(false);
        self.timer_ms = if self.motion || self.widget_host.is_some() { MOTION_MS } else { POLL_MS };
        if self.wallpaper.is_some() {
            self.show_window();
        }
        // Repaint with the new wallpaper.
        self.invalidate();
    }

    /// Fetch the media bytes for `path`, preferring the in-RAM cache so the
    /// disk is only touched when the file actually changed.
    fn media_data(&mut self, path: &Path) -> Option<(Arc<Vec<u8>>, Option<SystemTime>)> {
        let mtime = std::fs::metadata(path).and_then(|m| m.modified()).ok();
        if let Some((_, _, bytes)) = self
            .media_cache
            .as_ref()
            .filter(|(cp, cm, _)| *cp == path && Some(*cm) == mtime)
        {
            return Some((bytes.clone(), mtime));
        }
        let bytes = Arc::new(std::fs::read(path).ok()?);
        if let Some(m) = mtime {
            self.media_cache = Some((path.to_path_buf(), m, bytes.clone()));
        }
        Some((bytes, mtime))
    }

    /// The size videos should be decoded into — the desktop's bounding box, so
    /// we never convert or blit more pixels than are actually shown.
    fn target_size(&self) -> Option<(u32, u32)> {        if self.monitors.is_empty() {
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
        self.rebuild_widgets();
        self.invalidate();
    }

    /// (Re)create the widget canvas + scripts for the current desktop size.
    fn rebuild_widgets(&mut self) {
        if std::env::var("STRPAPER_NO_WIDGETS").is_ok() {
            return;
        }
        let size = (self.monitors.iter().map(|m| m.width).sum::<i32>().max(1) as u32,
                    self.monitors.iter().map(|m| m.height).sum::<i32>().max(1) as u32);
        self.widget_host =
            Some(widgets::WidgetHost::rebuild(size, &self.dir));
    }

    fn widgets_watcher_dirty(&mut self) -> bool {
        match &self.widgets_watcher {
            Some(w) => w.is_dirty(),
            None => false,
        }
    }

    /// Open a video player for pre-converted bytes (GPU first, SW fallback),
    /// applying the current pause state.
    fn open_video(&mut self, data: Arc<Vec<u8>>, target: Option<(u32, u32)>) {
        self.frame_start = Instant::now();
        self.painted_version = 0;
        let player = match video::VideoPlayer::open(data.clone(), target, true) {
            Ok(p) => p,
            Err(e) => {
                log(&format!("GPU decode unavailable ({e}); using software decode"));
                match video::VideoPlayer::open(data, target, false) {
                    Ok(p) => p,
                    Err(e) => {
                        log(&format!("video failed: {e}"));
                        self.wallpaper = None;
                        self.motion = false;
                        self.timer_ms = POLL_MS;
                        self.hide_window();
                        return;
                    }
                }
            }
        };
        let paused = self.render_paused;
        player.set_paused(paused);
        self.wallpaper = Some(Wallpaper::Video(player));
        self.motion = true;
        self.timer_ms = MOTION_MS;
        if !paused {
            self.invalidate();
        }
    }

    /// Apply any finished background pre-conversion.
    fn drain_transcodes(&mut self) {
        let Some(rx) = &self.transcode_rx else {
            return;
        };
        // Collect first so `self` can be mutated freely while applying.
        let mut results = Vec::new();
        while let Ok(msg) = rx.try_recv() {
            results.push(msg);
        }
        for msg in results {
            let generation = msg.generation();
            if generation != self.transcode_generation {
                continue; // superseded by a newer config/file change
            }
            match msg {
                TranscodeResult::Done(_, bytes) => {
                    // Cache the conversion so restarts/switches are instant.
                    if let (Some((p, m)), Some(qh)) =
                        (self.loaded_source.as_ref(), storage::read_configured_quality(&self.dir))
                    {
                        let out_h = qh & !1;
                        self.transcoded_cache =
                            Some(((p.clone(), *m, out_h), Arc::new(bytes.clone())));
                    }
                    let tgt = self.target_size();
                    self.open_video(Arc::new(bytes), tgt);
                }
                TranscodeResult::Failed(_, e) => {
                    log(&format!("pre-conversion failed ({e}); playing original"));
                    if let Some(path) = self.video_path.clone() {
                        if let Some((data, _)) = self.media_data(&path) {
                            let target = self.target_size();
                            self.frame_start = Instant::now();
                            self.painted_version = 0;
                            self.wallpaper = load_video(&data, target);
                            self.motion = self
                                .wallpaper
                                .as_ref()
                                .map(|w| needs_ticks(w))
                                .unwrap_or(false);
                            self.timer_ms = if self.motion || self.widget_host.is_some() { MOTION_MS } else { POLL_MS };
                            if self.wallpaper.is_some() {
                                self.show_window();
                            }
                            self.invalidate();
                        }
                    }
                }
            }
        }
    }

    /// One timer tick: pick up pending file changes and advance motion.
    fn tick(&mut self) {
        // Apply finished pre-conversions (even while covered).
        self.drain_transcodes();

        // Pause rendering while the desktop is completely covered by a
        // maximized/fullscreen application — nothing of it would be visible.
        let covered = desktop::any_window_covers_desktop();
        if covered != self.render_paused {
            self.render_paused = covered;
            if let Some(Wallpaper::Video(v)) = self.wallpaper.as_ref() {
                v.set_paused(covered);
            }
            log(if covered {
                "rendering paused (desktop fully covered)"
            } else {
                "rendering resumed"
            });
            if !covered {
                // Repaint immediately on resume (frame may be stale).
                self.painted_version = 0;
                self.invalidate();
            }
        }

        // Config / file changes are applied even while paused, so a wallpaper
        // swap is already in place when the desktop becomes visible again.
        if self.watcher_is_dirty() {
            std::thread::sleep(Duration::from_millis(SETTLE_MS));
            self.wallpaper = None;
            self.reload();
            if let Some(Wallpaper::Video(v)) = self.wallpaper.as_ref() {
                v.set_paused(self.render_paused);
            }
        }

        // Widget script changes: rebuild the canvas + scripts.
        if self.widgets_watcher_dirty() {
            self.rebuild_widgets();
            self.painted_version = 0;
            self.invalidate();
        }

        // Run widget scripts while visible (they feed the shared canvas).
        if !self.render_paused {
            if let Some(host) = &mut self.widget_host {
                host.render_tick();
                if host.has_changes() {
                    self.invalidate();
                }
            }
        }

        // Skip failure/stall checks and motion invalidation while covered.
        if self.render_paused {
            return;
        }

        // If the media could not be decoded (or stalled with no frames), fall
        // back: a GPU-mode player retries in software; software failure hides
        // the wallpaper so the desktop shows instead of freezing on grey.
        let (video_failed, reason, stalled, is_hw) = match self.wallpaper.as_ref() {
            Some(Wallpaper::Video(v)) => (
                v.is_failed(),
                v.failure_reason(),
                !v.has_yielded() && v.active_secs() > VIDEO_TIMEOUT_SECS,
                v.is_hw(),
            ),
            _ => (false, None, false, false),
        };
        let stall_reason = if stalled && reason.is_none() {
            Some(format!("no frames within {VIDEO_TIMEOUT_SECS}s"))
        } else {
            None
        };
        if video_failed || stalled {
            if let Some(r) = reason.as_deref().or(stall_reason.as_deref()) {
                log(&format!("video failed: {r}"));
            }
            // A GPU-mode failure may be specific to hardware decode; retry the
            // same file with the CPU decoder before giving up.
            if is_hw {
                if let Some(path) = self.video_path.clone() {
                    log("video: retrying with software decode");
                    let target = self.target_size();
                    if let Some((data, _)) = self.media_data(&path) {
                        self.frame_start = Instant::now();
                        self.painted_version = 0;
                        self.wallpaper = video::VideoPlayer::open(data, target, false)
                            .map(Wallpaper::Video)
                            .ok();
                        self.motion = self
                            .wallpaper
                            .as_ref()
                            .map(|w| needs_ticks(w))
                            .unwrap_or(false);
                        self.timer_ms = if self.motion || self.widget_host.is_some() { MOTION_MS } else { POLL_MS };
                        if self.wallpaper.is_some() {
                            self.show_window();
                            self.invalidate();
                            return;
                        }
                    }
                }
            }
            self.wallpaper = None;
            self.video_path = None;
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
                // Composite widgets over the wallpaper frame when the widget
                // canvas matches the raster dimensions (both are desktop-sized
                // for videos and pre-fitted stills).
                let mut composited: Option<render::Raster> = None;
                if let Some(host) = &mut self.widget_host {
                    if let Some((cw, ch)) = host.canvas_dims() {
                        if raster.width == cw && raster.height == ch {
                            let mut frame = raster.bgra.clone();
                            host.composite_pending(frame.as_mut_slice());
                            composited = Some(render::Raster {
                                width: cw,
                                height: ch,
                                bgra: frame,
                            });
                        }
                    }
                }
                match &composited {
                    Some(frame) => {
                        render::paint_frame(hdc, &self.monitors, self.origin, frame);
                    }
                    None => {
                        render::paint_frame(hdc, &self.monitors, self.origin, raster.as_ref());
                    }
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
            log("RegisterClassW failed");
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
        app.rebuild_widgets();

        // Widget canvas + scripts are built by rebuild_widgets() (called from
        // refresh paths); nothing window-based to do here.

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

/// Open a video from an in-memory copy, GPU decoding first with automatic
/// fallback to software if the GPU pipeline cannot be set up.
fn load_video(data: &Arc<Vec<u8>>, target: Option<(u32, u32)>) -> Option<Wallpaper> {
    match video::VideoPlayer::open(data.clone(), target, true) {
        Ok(p) => Some(Wallpaper::Video(p)),
        Err(e) => {
            log(&format!("GPU decode unavailable ({e}); using software decode"));
            video::VideoPlayer::open(data.clone(), target, false)
                .map(Wallpaper::Video)
                .map_err(|e| format!("video: {e}"))
                .ok()
        }
    }
}
