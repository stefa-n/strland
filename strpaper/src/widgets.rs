//! Programmable desktop widgets.
//!
//! Every `.rhai` file in `%USERPROFILE%\.strland\strpaper\widgets\` contributes a
//! `draw(pen)` function. All scripts draw into one shared desktop-sized
//! canvas; damaged regions are detected, alpha-composited over the wallpaper
//! frame, and only repainted when something visibly changed. Widgets live
//! *inside* the wallpaper window: above the wallpaper, below icons/apps.
//!
//! ```rhai
//! fn draw(pen) {
//!     pen.clear();
//!     pen.text(24, 24, "Hello!", 28, "#FFFFFF");
//! }
//!
//! // Optional: pick the update rate for this widget (1-120, default 30).
//! fn fps() { 5 }
//! ```
//!
//! Pen API: `clear`, `fill_rect(x,y,w,h,color)`, `line(x1,y1,x2,y2,w,color)`,
//! `text(x,y,str,size,color)`, `width`, `height`, `cpu`, `ram`, `time(fmt)`.
//! Colours are `"#RGB"`, `"#RRGGBB"` or `"#RRGGBBAA"`; alpha blends over the
//! wallpaper.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use rhai::{Engine, Scope};

/// Fixed virtual canvas that all widget scripts draw into.  Every script
/// sees `width() = VIRTUAL_W` and `height() = VIRTUAL_H` regardless of
/// the actual monitor resolution.  The pen scales coordinates to the
/// physical buffer automatically.
const VIRTUAL_W: usize = 2880;
const VIRTUAL_H: usize = 1620;

/// Default frames per second a widget re-runs its script at. Scripts can
/// override this per widget with an optional `fn fps()` (clamped 1-120).
const WIDGET_FPS: u64 = 30;

pub const WIDGETS_DIR_NAME: &str = "widgets";

/// Full path of the widgets directory (created if missing).
pub fn widgets_dir(wallpaper_dir: &std::path::Path) -> std::path::PathBuf {
    let dir = wallpaper_dir.join(WIDGETS_DIR_NAME);
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Returns the system DPI scale factor (1.0 = 96 DPI, 1.25 = 120 DPI, etc.).
pub(crate) fn dpi_scale() -> f64 {
    static CACHE: OnceLock<f64> = OnceLock::new();
    *CACHE.get_or_init(|| {
        unsafe {
            use windows::Win32::UI::HiDpi::GetDpiForSystem;
            GetDpiForSystem() as f64 / 96.0
        }
    })
}

/// Write an example widget so the feature is discoverable.
pub fn ensure_sample(dir: &std::path::Path) {
    let sample = dir.join("clock.rhai");
    let empty = std::fs::read_dir(dir)
        .map(|mut d| d.next().is_none())
        .unwrap_or(true);
    if empty && !sample.exists() {
        let _ = std::fs::write(
            &sample,
            r###"// strpaper widget — runs ~30x per second.
// Draw anywhere on the desktop with the `pen`.

fn draw(pen) {
    pen.clear();

    // Clock
    pen.text(48, 48, pen.time("%H:%M:%S"), 42, "#FFFFFF");

    // CPU bar
    let cpu = pen.cpu();
    pen.fill_rect(48, 110, 260, 10, "#333333");
    pen.fill_rect(48, 110, 260 * cpu / 100, 10, "#7AA2F7");

    // RAM bar
    let ram = pen.ram();
    pen.fill_rect(48, 130, 260, 10, "#333333");
    pen.fill_rect(48, 130, 260 * ram / 100, 10, "#9ECE6A");

    pen.text(48, 152, "cpu " + cpu + "%   ram " + ram + "%", 16, "#A9B1D6");
}
"###,
        );
    }
}

// ---------------------------------------------------------------------------
// Pen
// ---------------------------------------------------------------------------

/// Shared drawing surface handed to scripts (premultiplied BGRA, top-down).
#[derive(Clone)]
pub struct Pen {
    inner: Arc<Mutex<PenSurface>>,
}

#[derive(Default)]
struct PenSurface {
    w: usize,
    h: usize,
    pixels: Vec<u8>,
    /// Damage rectangle since the last snapshot: (x0, y0, x1, y1).
    bbox: Option<(usize, usize, usize, usize)>,
    /// Global opacity applied when the frame is captured (0.0–1.0).
    opacity: f32,
    /// DPI scale factor (e.g. 1.5 for 150%). Coordinates from scripts are
    /// multiplied by this before touching pixels, so scripts use logical
    /// coordinates and everything renders at physical resolution.
    scale: f64,
    /// Persistent key-value store that survives across `call_fn` invocations.
    state: HashMap<String, rhai::Dynamic>,
}

impl Pen {
    fn new() -> Pen {
        Pen {
            inner: Arc::new(Mutex::new(PenSurface::default())),
        }
    }

    fn set_scale(&self, sc: f64) {
        self.inner.lock().unwrap().scale = sc;
    }

    fn set_region(&self, w: usize, h: usize) {
        let mut s = self.inner.lock().unwrap();
        if s.w != w || s.h != h {
            s.w = w;
            s.h = h;
            let pw = (w as f64 * s.scale).ceil() as usize;
            let ph = (h as f64 * s.scale).ceil() as usize;
            s.pixels = vec![0u8; pw * ph * 4];
            s.bbox = Some((0, 0, pw, ph));
        }
    }

    /// Logical `(w, h)` — what scripts see.
    /// Always returns the fixed virtual canvas size.
    pub fn dims(&self) -> (usize, usize) {
        (VIRTUAL_W, VIRTUAL_H)
    }

    /// Physical pixel dimensions (logical × scale).
    fn phys(&self) -> (usize, usize) {
        let s = self.inner.lock().unwrap();
        let pw = (s.w as f64 * s.scale).ceil() as usize;
        let ph = (s.h as f64 * s.scale).ceil() as usize;
        (pw, ph)
    }

    /// Read a value from persistent key-value state (survives across `call_fn` calls).
    /// Returns `0` if the key has not been set.
    fn state(&self, key: &str) -> rhai::Dynamic {
        let s = self.inner.lock().unwrap();
        s.state.get(key).cloned().unwrap_or(rhai::Dynamic::from(0_i64))
    }

    /// Write a value to persistent key-value state.
    fn set_state(&self, key: &str, val: rhai::Dynamic) {
        let mut s = self.inner.lock().unwrap();
        s.state.insert(key.to_string(), val);
    }

    fn mark(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let mut s = self.inner.lock().unwrap();
        s.bbox = match (s.bbox, (x0, y0, x1, y1)) {
            (None, cur) => Some(cur),
            (Some((a0, a1, a2, a3)), (b0, b1, b2, b3)) => Some((
                a0.min(b0),
                a1.min(b1),
                a2.max(b2),
                a3.max(b3),
            )),
        };
    }

    /// Snapshot the canvas bytes and clear damage tracking. Returns
    /// `(x0, y0, x1, y1, bytes)` or `None` when nothing was drawn.
    fn take_frame(&mut self) -> Option<(usize, usize, usize, usize, Vec<u8>)> {
        let mut s = self.inner.lock().unwrap();
        let (x0, y0, x1, y1) = s.bbox?;
        s.bbox = None;
        if s.pixels.is_empty() || x1 <= x0 || y1 <= y0 {
            return None;
        }
        let pw = (s.w as f64 * s.scale).ceil() as usize;
        let row = (x1 - x0) * 4;
        let mut out = Vec::with_capacity(row * (y1 - y0));
        let opa = s.opacity;
        for y in y0..y1 {
            let off = (y * pw + x0) * 4;
            let slice = &s.pixels[off..off + row];
            if opa < 1.0 {
                for px in slice.chunks_exact(4) {
                    out.push((px[0] as f32 * opa) as u8);
                    out.push((px[1] as f32 * opa) as u8);
                    out.push((px[2] as f32 * opa) as u8);
                    out.push((px[3] as f32 * opa) as u8);
                }
            } else {
                out.extend_from_slice(slice);
            }
        }
        Some((x0, y0, x1, y1, out))
    }

    /// Read the current pixel data at a given region without modifying any
    /// state (unlike `take_frame` which resets `bbox`). Used by the host to
    /// re-composite non-due widgets when any widget changes.
    fn peek_frame(&self, bx0: usize, by0: usize, bx1: usize, by1: usize) -> Option<Vec<u8>> {
        let s = self.inner.lock().unwrap();
        if s.pixels.is_empty() || bx1 <= bx0 || by1 <= by0 {
            return None;
        }
        let pw = (s.w as f64 * s.scale).ceil() as usize;
        let ph = (s.h as f64 * s.scale).ceil() as usize;
        let bx0 = bx0.min(pw);
        let by0 = by0.min(ph);
        let bx1 = bx1.min(pw);
        let by1 = by1.min(ph);
        if bx1 <= bx0 || by1 <= by0 {
            return None;
        }
        let row = (bx1 - bx0) * 4;
        let mut out = Vec::with_capacity(row * (by1 - by0));
        let opa = s.opacity;
        for y in by0..by1 {
            let off = (y * pw + bx0) * 4;
            let slice = &s.pixels[off..off + row];
            if opa < 1.0 {
                for px in slice.chunks_exact(4) {
                    out.push((px[0] as f32 * opa) as u8);
                    out.push((px[1] as f32 * opa) as u8);
                    out.push((px[2] as f32 * opa) as u8);
                    out.push((px[3] as f32 * opa) as u8);
                }
            } else {
                out.extend_from_slice(slice);
            }
        }
        Some(out)
    }

    /// Blend one premultiplied pixel (src-over) with bounds checking.
    fn blend_px(&mut self, x: usize, y: usize, premul: [u8; 4]) {
        let mut s = self.inner.lock().unwrap();
        let pw = (s.w as f64 * s.scale).ceil() as usize;
        let ph = (s.h as f64 * s.scale).ceil() as usize;
        if x >= pw || y >= ph {
            return;
        }
        let o = (y * pw + x) * 4;
        if o + 3 >= s.pixels.len() {
            return;
        }
        let a = premul[3] as u32;
        let inv = 255 - a;
        s.pixels[o] = (premul[0] as u32 + s.pixels[o] as u32 * inv / 255).min(255) as u8;
        s.pixels[o + 1] = (premul[1] as u32 + s.pixels[o + 1] as u32 * inv / 255).min(255) as u8;
        s.pixels[o + 2] = (premul[2] as u32 + s.pixels[o + 2] as u32 * inv / 255).min(255) as u8;
        s.pixels[o + 3] = (a + s.pixels[o + 3] as u32 * inv / 255).min(255) as u8;
    }

    /// Solid premultiplied fill (src-over) with damage marking.
    fn fill_solid(&mut self, x: i64, y: i64, w: i64, h: i64, rgba: [u8; 4]) {
        let (sc, lw, lh) = { let s = self.inner.lock().unwrap(); (s.scale, s.w, s.h) };
        let pw = (lw as f64 * sc).ceil() as i64;
        let ph = (lh as f64 * sc).ceil() as i64;
        let x0 = ((x as f64 * sc) as i64).max(0);
        let y0 = ((y as f64 * sc) as i64).max(0);
        let x1 = (((x + w) as f64 * sc).ceil() as i64).min(pw);
        let y1 = (((y + h) as f64 * sc).ceil() as i64).min(ph);
        if x1 <= x0 || y1 <= y0 {
            return;
        }
        self.mark(x0 as usize, y0 as usize, x1 as usize, y1 as usize);
        let sa = rgba[3] as u32;
        let inv = 255 - sa;
        let pm_b = ((rgba[2] as u32 * sa) / 255) as u32;
        let pm_g = ((rgba[1] as u32 * sa) / 255) as u32;
        let pm_r = ((rgba[0] as u32 * sa) / 255) as u32;
        let mut s = self.inner.lock().unwrap();
        let pw = pw as usize;
        for yy in y0 as usize..y1 as usize {
            let off = yy * pw + x0 as usize;
            for xx in 0..(x1 - x0) as usize {
                let o = (off + xx) * 4;
                s.pixels[o] = (pm_b + s.pixels[o] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 1] = (pm_g + s.pixels[o + 1] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 2] = (pm_r + s.pixels[o + 2] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 3] = (sa + s.pixels[o + 3] as u32 * inv / 255).min(255) as u8;
            }
        }
    }
}

// Script-facing methods.
impl Pen {
    /// Draw an arc (partial circle outline) centred at (cx, cy).
    /// `start_deg` and `sweep_deg` are in degrees; 0° = right, 90° = down.
    pub fn arc(&mut self, cx: i64, cy: i64, r: i64, start_deg: f64, sweep_deg: f64, thickness: i64, c: [u8; 4]) {
        let steps = ((sweep_deg.abs() as i64).max(4) * 2) as usize;
        let half = (thickness.max(1) / 2) as i64;
        let rf = r as f64;
        for i in 0..steps {
            let angle = (start_deg + sweep_deg * (i as f64 / steps as f64)).to_radians();
            let px = cx + (rf * angle.cos()) as i64;
            let py = cy + (rf * angle.sin()) as i64;
            self.fill_solid(px - half, py - half, thickness.max(1), thickness.max(1), c);
        }
    }

    /// Draw a circle outline.
    pub fn circle(&mut self, cx: i64, cy: i64, r: i64, thickness: i64, c: [u8; 4]) {
        self.arc(cx, cy, r, 0.0, 360.0, thickness, c);
    }

    /// Draw a filled circle.
    pub fn fill_circle(&mut self, cx: i64, cy: i64, r: i64, c: [u8; 4]) {
        let sc = { self.inner.lock().unwrap().scale };
        let scx = (cx as f64 * sc) as i64;
        let scy = (cy as f64 * sc) as i64;
        let sr = (r as f64 * sc).ceil() as i64;
        let rf = sr as f64;
        for dy in -sr..=sr {
            for dx in -sr..=sr {
                if (dx * dx + dy * dy) as f64 <= rf * rf {
                    let px = (scx + dx) as usize;
                    let py = (scy + dy) as usize;
                    self.blend_px(px, py, premul_bgra(c));
                }
            }
        }
    }

    /// Draw a rounded rectangle (filled).
    pub fn fill_round_rect(&mut self, x: i64, y: i64, w: i64, h: i64, radius: i64, c: [u8; 4]) {
        if w <= 0 || h <= 0 || radius <= 0 {
            return;
        }
        let (sc, lw, lh) = { let s = self.inner.lock().unwrap(); (s.scale, s.w, s.h) };
        let pw = (lw as f64 * sc).ceil() as i64;
        let ph = (lh as f64 * sc).ceil() as i64;
        // Main body
        self.fill_solid(x, y + radius, w, h - radius * 2, c);
        // Top and bottom strips
        self.fill_solid(x + radius, y, w - radius * 2, radius, c);
        self.fill_solid(x + radius, y + h - radius, w - radius * 2, radius, c);
        // Corner arcs with anti-aliased edges.
        let rf = radius as f64;
        let mut corners: Vec<(i64, i64, u8)> = Vec::new();
        for dy in -radius..=radius {
            for dx in -radius..=radius {
                let dist = ((dx * dx + dy * dy) as f64).sqrt();
                if dist > rf + 1.0 {
                    continue;
                }
                let alpha = if dist <= rf - 1.0 {
                    1.0
                } else if dist <= rf + 1.0 {
                    (rf + 1.0 - dist) / 2.0
                } else {
                    continue;
                };
                let corner_a = (c[3] as f64 * alpha) as u8;
                if corner_a == 0 {
                    continue;
                }
                let (cx, cy) = if dx < 0 && dy < 0 {
                    (x + radius + dx, y + radius + dy)
                } else if dx >= 0 && dy < 0 {
                    (x + w - 1 - radius + dx, y + radius + dy)
                } else if dx < 0 && dy >= 0 {
                    (x + radius + dx, y + h - 1 - radius + dy)
                } else {
                    (x + w - 1 - radius + dx, y + h - 1 - radius + dy)
                };
                corners.push((cx, cy, corner_a));
            }
        }
        if corners.is_empty() {
            return;
        }
        let mut s = self.inner.lock().unwrap();
        for (cx, cy, ca) in &corners {
            let x0 = ((*cx as f64 * sc) as i64).max(0);
            let y0 = ((*cy as f64 * sc) as i64).max(0);
            let x1 = (((*cx + 1) as f64 * sc).ceil() as i64).max(x0 + 1).min(pw);
            let y1 = (((*cy + 1) as f64 * sc).ceil() as i64).max(y0 + 1).min(ph);
            let a = *ca as u32;
            let inv = 255 - a;
            let pr = ((c[2] as u32 * a) / 255) as u8;
            let pg = ((c[1] as u32 * a) / 255) as u8;
            let pb = ((c[0] as u32 * a) / 255) as u8;
            for yy in y0 as usize..y1 as usize {
                for xx in x0 as usize..x1 as usize {
                    let o = (yy * pw as usize + xx) * 4;
                    if o + 3 < s.pixels.len() {
                        s.pixels[o] = (pb as u32 + s.pixels[o] as u32 * inv / 255).min(255) as u8;
                        s.pixels[o+1] = (pg as u32 + s.pixels[o+1] as u32 * inv / 255).min(255) as u8;
                        s.pixels[o+2] = (pr as u32 + s.pixels[o+2] as u32 * inv / 255).min(255) as u8;
                        s.pixels[o+3] = (a + s.pixels[o+3] as u32 * inv / 255).min(255) as u8;
                    }
                }
            }
        }
        drop(s);
        let bx0 = corners.iter().map(|(cx, _, _)| ((*cx as f64 * sc) as i64).max(0) as usize).min().unwrap();
        let by0 = corners.iter().map(|(_, cy, _)| ((*cy as f64 * sc) as i64).max(0) as usize).min().unwrap();
        let bx1 = corners.iter().map(|(cx, _, _)| (((*cx + 1) as f64 * sc).ceil() as i64).min(pw) as usize).max().unwrap();
        let by1 = corners.iter().map(|(_, cy, _)| (((*cy + 1) as f64 * sc).ceil() as i64).min(ph) as usize).max().unwrap();
        self.mark(bx0, by0, bx1, by1);
    }

    pub fn clear(&mut self) {
        let mut s = self.inner.lock().unwrap();
        s.pixels.iter_mut().for_each(|b| *b = 0);
        let pw = (s.w as f64 * s.scale).ceil() as usize;
        let ph = (s.h as f64 * s.scale).ceil() as usize;
        s.bbox = Some((0, 0, pw, ph));
        s.opacity = 1.0;
    }

    /// Set global opacity (0.0–1.0) applied to every pixel when the frame is
    /// captured. Call at the top of `draw()` before any drawing.
    pub fn set_opacity(&self, a: f32) {
        self.inner.lock().unwrap().opacity = a.clamp(0.0, 1.0);
    }

    fn line(&mut self, x1: i64, y1: i64, x2: i64, y2: i64, thickness: i64, c: [u8; 4]) {
        let sx = if x1 < x2 { 1 } else { -1 };
        let sy = if y1 < y2 { 1 } else { -1 };
        let dx = (x2 - x1).abs();
        let dy = (y2 - y1).abs();
        let mut err = dx - dy;
        let (mut x, mut y) = (x1, y1);
        let t = thickness.max(1);
        let half = t / 2;
        loop {
            self.fill_solid(x - half, y - half, t, t, c);
            if x == x2 && y == y2 {
                break;
            }
            let e2 = err * 2;
            if e2 > -dy {
                err -= dy;
                x += sx;
            }
            if e2 < dx {
                err += dx;
                y += sy;
            }
        }
    }

    fn text(&mut self, x: i64, y: i64, text: &str, px: i64, c: [u8; 4]) {
        draw_text_buffer(self, x, y, text, px, c, "Segoe UI", 0);
    }

    fn text_font(&mut self, x: i64, y: i64, text: &str, px: i64, c: [u8; 4], font: &str) {
        draw_text_buffer(self, x, y, text, px, c, font, 0);
    }

    fn text_font_spacing(&mut self, x: i64, y: i64, text: &str, px: i64, c: [u8; 4], font: &str, spacing: i64) {
        draw_text_buffer(self, x, y, text, px, c, font, spacing);
    }

    fn cpu_percent(&mut self) -> i64 {
        cpu_usage()
    }

    fn ram_percent(&mut self) -> i64 {
        unsafe {
            use windows::Win32::System::SystemInformation::{
                GlobalMemoryStatusEx, MEMORYSTATUSEX,
            };
            let mut ms = MEMORYSTATUSEX::default();
            ms.dwLength = std::mem::size_of::<MEMORYSTATUSEX>() as u32;
            if GlobalMemoryStatusEx(&mut ms).is_ok() && ms.ullTotalPhys > 0 {
                ((ms.ullTotalPhys - ms.ullAvailPhys) * 100 / ms.ullTotalPhys) as i64
            } else {
                0
            }
        }
    }

    fn battery(&mut self) -> i64 {
        crate::sysdata::battery().percent
    }

    fn charging(&mut self) -> bool {
        crate::sysdata::battery().charging
    }

    fn bt_count(&mut self) -> i64 {
        crate::sysdata::bt_devices().len() as i64
    }

    fn bt_level(&mut self, index: i64) -> i64 {
        let devices = crate::sysdata::bt_devices();
        devices.get(index as usize).map(|d| d.level).unwrap_or(-1)
    }

    fn bt_name(&mut self, index: i64) -> String {
        let devices = crate::sysdata::bt_devices();
        devices.get(index as usize).map(|d| d.name.clone()).unwrap_or_default()
    }

    fn audio_level(&mut self, pos: f64) -> i64 {
        let bands = crate::sysdata::audio_bands();
        let idx = (pos.clamp(0.0, 1.0) * (bands.len() - 1) as f64).round() as usize;
        (bands[idx].clamp(0.0, 1.0) * 100.0) as i64
    }

    fn media_playing(&mut self) -> bool {
        crate::sysdata::media_info().playing
    }

    #[allow(dead_code)]
    fn media_title(&mut self) -> String {
        crate::sysdata::media_info().title
    }

    #[allow(dead_code)]
    fn media_artist(&mut self) -> String {
        crate::sysdata::media_info().artist
    }

    fn time(&mut self, fmt: &str) -> String {
        unsafe {
            use windows::Win32::System::SystemInformation::GetLocalTime;
            format_datetime(GetLocalTime(), fmt)
        }
    }

    /// Formatted local date. Without an argument this yields the short
    /// weekday/month form, e.g. `Mon, Aug 24`.
    fn date(&mut self, fmt: &str) -> String {
        unsafe {
            use windows::Win32::System::SystemInformation::GetLocalTime;
            format_datetime(GetLocalTime(), fmt)
        }
    }

    /// Blocking HTTP GET with 60-second caching per URL.
    fn http_get_impl(&self, url: &str) -> String {
        http_get_cached(url)
    }

    /// Download binary content from a URL to a local file.
    fn http_download_impl(&self, url: &str, save_path: &str) -> bool {
        let resolved = resolve_widget_path(save_path);
        if let Some(parent) = resolved.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        match http_download_to(url, &resolved) {
            Ok(()) => true,
            Err(e) => {
                crate::logger::log(&format!("http_download {url}: {e}"));
                false
            }
        }
    }

    /// Run a shell command and return trimmed stdout.
    fn exec_impl(&self, cmd: &str) -> String {
        exec_command(cmd)
    }

    /// Load an image from disk (or wallpaper-relative path), resize to
    /// `(w x h)` and alpha-blend onto the canvas at `(x, y)`.
    fn draw_image_impl(&mut self, x: i64, y: i64, w: i64, h: i64, path: &str) {
        let sc = { self.inner.lock().unwrap().scale };
        let pw = (w as f64 * sc).ceil() as usize;
        let ph = (h as f64 * sc).ceil() as usize;
        if pw == 0 || ph == 0 {
            return;
        }
        let resolved = resolve_widget_path(path);
        if let Some((sw, sh, rgba_data)) = load_image_cached(&resolved) {
            let resized = resize_rgba(&rgba_data, sw as usize, sh as usize, pw, ph);
            blend_rgba_at(self, x, y, pw, ph, &resized);
        }
    }

    /// Render an SVG file scaled to `(w x h)` and alpha-blend it at `(x, y)`.
    fn draw_svg_impl(&mut self, x: i64, y: i64, w: i64, h: i64, path: &str) {
        if w <= 0 || h <= 0 {
            return;
        }
        let sc = { self.inner.lock().unwrap().scale };
        let pw = (w as f64 * sc).ceil() as u32;
        let ph = (h as f64 * sc).ceil() as u32;
        if pw == 0 || ph == 0 {
            return;
        }
        let resolved = resolve_widget_path(path);
        if let Some(rgba) = render_svg_cached(&resolved, pw, ph) {
            blend_rgba_at(self, x, y, pw as usize, ph as usize, &rgba);
        }
    }

    /// Draw the current frame of a video file at `(x, y)` scaled to
    /// `(w x h)`. The file is decoded continuously on a background thread
    /// (one player per path), so this always shows live playback.
    fn draw_video_impl(&mut self, x: i64, y: i64, w: i64, h: i64, path: &str) {
        if w <= 0 || h <= 0 {
            return;
        }
        let sc = { self.inner.lock().unwrap().scale };
        let pw = (w as f64 * sc).ceil() as usize;
        let ph = (h as f64 * sc).ceil() as usize;
        if pw == 0 || ph == 0 {
            return;
        }
        let resolved = resolve_widget_path(path);
        // Decode at (up to) the requested size so no pixels are wasted.
        let target = Some((pw.min(3840 * 2).max(2) as u32, ph.min(2160 * 2).max(2) as u32));
        let Some(player) = video_player_for(&resolved, target) else {
            return;
        };
        let Some(raster) = player.frame_at(Duration::ZERO) else {
            return;
        };
        if raster.width == 0 || raster.height == 0 || raster.bgra.is_empty() {
            return;
        }
        // BGRA -> RGBA for the shared resize/blend helpers.
        let mut rgba = Vec::with_capacity(raster.bgra.len());
        for px in raster.bgra.chunks_exact(4) {
            rgba.extend_from_slice(&[px[2], px[1], px[0], 255]);
        }
        let resized = resize_rgba(&rgba, raster.width, raster.height, pw, ph);
        blend_rgba_at(self, x, y, pw, ph, &resized);
    }

    fn gpu_percent(&mut self) -> i64 {
        crate::sysdata::gpu_usage()
    }

    // -- Regex -----------------------------------------------------------------

    /// Returns true if `text` matches the regex `pattern`.
    fn regex_match(pattern: &str, text: &str) -> bool {
        regex::Regex::new(pattern)
            .ok()
            .map_or(false, |re| re.is_match(text))
    }

    /// Returns the first match of `pattern` in `text`, or empty string.
    fn regex_find(pattern: &str, text: &str) -> String {
        let Ok(re) = regex::Regex::new(pattern) else {
            return String::new();
        };
        re.captures(text)
            .and_then(|c| c.get(1).or_else(|| c.get(0)))
            .map(|m| m.as_str().to_string())
            .unwrap_or_default()
    }

    /// Replaces the first (or all) occurrences of `pattern` in `text`.
    fn regex_replace(pattern: &str, text: &str, replacement: &str, all: bool) -> String {
        let Ok(re) = regex::Regex::new(pattern) else {
            return text.to_string();
        };
        if all {
            re.replace_all(text, replacement).into_owned()
        } else {
            re.replace(text, replacement).into_owned()
        }
    }

    // -- File I/O --------------------------------------------------------------

    /// Read a file to a string. Path is resolved relative to the widget dir.
    fn read_file(path: &str) -> String {
        let resolved = resolve_widget_path(path);
        std::fs::read_to_string(&resolved).unwrap_or_default()
    }

    /// Write `content` to a file. Creates parent directories if needed.
    /// Returns true on success.
    fn write_file(path: &str, content: &str) -> bool {
        let resolved = resolve_widget_path(path);
        if let Some(parent) = resolved.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&resolved, content).is_ok()
    }

    /// Returns true if `path` exists.
    fn file_exists(path: &str) -> bool {
        resolve_widget_path(path).exists()
    }
}

// ---------------------------------------------------------------------------
// Host
// ---------------------------------------------------------------------------

struct ScriptItem {
    name: String,
    engine: Engine,
    ast: rhai::AST,
    /// Throttle interval for this widget (from its optional `fn fps()`).
    interval: Duration,
    last_run: Instant,
    /// This widget's own drawing surface (kept between frames so its last
    /// output stays visible while the script idles).
    pen: Pen,
    /// Hash of this widget's most recent output.
    hash: u64,
    /// True right after this widget produced pixels different from before.
    dirty: bool,
    /// Last accumulated bounding box in the combined buffer, so we can erase
    /// it before re-accumulating (prevents alpha buildup for semi-transparent
    /// widgets).
    last_bbox: Option<(usize, usize, usize, usize)>,
}

/// Runs widget scripts into per-widget canvases and accumulates their output
/// into one desktop-sized buffer that is composited over the wallpaper.
pub struct WidgetHost {
    width: usize,
    height: usize,
    /// Combined premultiplied-BGRA output of every widget (alpha starts 0).
    buf: Vec<u8>,
    items: Vec<ScriptItem>,
    /// Set when any widget produced pixels that differ from its previous
    /// output (used to skip redundant repaints).
    changed: bool,
}

impl WidgetHost {
    /// `Some((w, h))` when the canvas is allocated.
    pub fn canvas_dims(&self) -> Option<(usize, usize)> {
        if self.width == 0 || self.height == 0 {
            None
        } else {
            Some((self.width, self.height))
        }
    }

    /// Number of loaded scripts.
    pub fn script_count(&self) -> usize {
        self.items.len()
    }

    /// True when visible output differs from what is currently displayed.
    pub fn has_changes(&self) -> bool {
        self.changed
    }

    /// Run each script when its own throttle interval elapses (scripts can
    /// pick their rate with an optional `fn fps()`; default [`WIDGET_FPS`]).
    ///
    /// Every script owns a private surface, so a widget's drawing stays on
    /// screen unchanged while it idles — only the widgets whose code actually
    /// re-runs produce new pixels.
    ///
    /// When any widget changes, the dirty region (union of all changed widgets'
    /// old + new bounding boxes) is cleared **once**, then **all** widgets are
    /// re-composited into it. This prevents one widget's `clear_buf_region`
    /// call from erasing another widget's freshly accumulated pixels.
    pub fn render_tick(&mut self) {
        if self.items.is_empty() {
            return;
        }
        let now = Instant::now();

        let due: Vec<usize> = self
            .items
            .iter()
            .enumerate()
            .filter(|(_, it)| now.duration_since(it.last_run) >= it.interval)
            .map(|(i, _)| i)
            .collect();
        if due.is_empty() {
            return;
        }

        let mut scope = Scope::new();

        // --- Pass 1: run all due scripts and collect new frames ---------------
        let mut new_frames: Vec<Option<(usize, usize, usize, usize, Vec<u8>)>> =
            vec![None; self.items.len()];
        let mut changed = Vec::new();

        for &i in &due {
            let frame = {
                let item = &mut self.items[i];
                item.last_run = now;
                item.dirty = false;
                item.pen.clear();
                let pen = item.pen.clone();
                if let Err(e) =
                    item.engine.call_fn::<()>(&mut scope, &item.ast, "draw", (pen,))
                {
                    crate::logger::log(&format!("widget {}: script error: {e}", item.name));
                }
                item.pen.take_frame()
            };
            if let Some((x0, y0, x1, y1, ref bytes)) = frame {
                let hash = fnv_hash(bytes);
                if hash != self.items[i].hash {
                    changed.push(i);
                }
                new_frames[i] = Some((x0, y0, x1, y1, bytes.clone()));
            }
        }

        if changed.is_empty() {
            return;
        }

        // --- Pass 2: compute the union dirty region ---------------------------
        let mut dirty = (usize::MAX, usize::MAX, 0usize, 0usize);
        for &i in &changed {
            // Old bbox (what this widget previously contributed).
            if let Some((bx0, by0, bx1, by1)) = self.items[i].last_bbox {
                dirty.0 = dirty.0.min(bx0);
                dirty.1 = dirty.1.min(by0);
                dirty.2 = dirty.2.max(bx1);
                dirty.3 = dirty.3.max(by1);
            }
            // New bbox.
            if let Some((x0, y0, x1, y1, _)) = &new_frames[i] {
                dirty.0 = dirty.0.min(*x0);
                dirty.1 = dirty.1.min(*y0);
                dirty.2 = dirty.2.max(*x1);
                dirty.3 = dirty.3.max(*y1);
            }
        }
        if dirty.0 >= dirty.2 || dirty.1 >= dirty.3 {
            return;
        }

        // --- Pass 3: clear the dirty region once ------------------------------
        self.clear_buf_region(dirty.0, dirty.1, dirty.2, dirty.3);

        // --- Pass 4: re-accumulate ALL widgets into the dirty region ----------
        for i in 0..self.items.len() {
            // Prefer the freshly rendered frame if available; otherwise peek.
            if let Some((x0, y0, x1, y1, bytes)) = &new_frames[i] {
                self.accumulate(*x0, *y0, *x1, *y1, bytes);
                let item = &mut self.items[i];
                item.hash = fnv_hash(bytes);
                item.dirty = true;
                item.last_bbox = Some((*x0, *y0, *x1, *y1));
                self.changed = true;
            } else if let Some((bx0, by0, bx1, by1)) = self.items[i].last_bbox {
                // Non-due widget with existing output — only re-composite
                // if its bbox touches the dirty region.
                if bx0 < dirty.2 && bx1 > dirty.0 && by0 < dirty.3 && by1 > dirty.1 {
                    if let Some(bytes) = self.items[i].pen.peek_frame(bx0, by0, bx1, by1) {
                        self.accumulate(bx0, by0, bx1, by1, &bytes);
                        self.changed = true;
                    }
                }
            }
        }
    }

    /// True when the combined buffer holds no pixels in the given region
    /// (i.e. this is the widget's very first output there).
    fn buf_is_blank_at(&self, x0: usize, y0: usize, x1: usize, y1: usize) -> bool {
        for dy in y0..y1.min(self.height) {
            for dx in x0..x1.min(self.width) {
                let o = (dy * self.width + dx) * 4 + 3;
                if o < self.buf.len() && self.buf[o] != 0 {
                    return false;
                }
            }
        }
        true
    }

    /// Zero out a rectangular region in the combined buffer so a widget can
    /// re-accumulate without alpha buildup.
    fn clear_buf_region(&mut self, x0: usize, y0: usize, x1: usize, y1: usize) {
        let cw = self.width;
        for dy in y0..y1.min(self.height) {
            for dx in x0..x1.min(cw) {
                let o = (dy * cw + dx) * 4;
                if o + 4 <= self.buf.len() {
                    self.buf[o] = 0;
                    self.buf[o + 1] = 0;
                    self.buf[o + 2] = 0;
                    self.buf[o + 3] = 0;
                }
            }
        }
    }

    /// Blend one widget's premultiplied-BGRA region into the combined buffer.
    fn accumulate(&mut self, x0: usize, y0: usize, x1: usize, y1: usize, bytes: &[u8]) {
        let cw = self.width;
        let row = x1 - x0;
        let mut i = 0usize;
        for ri in 0..(y1 - y0) {
            for ci in 0..row {
                let s0 = i;
                i += 4;
                let dx = x0 + ci;
                let dy = y0 + ri;
                if dx >= cw {
                    continue;
                }
                let d = (dy * cw + dx) * 4;
                if d + 4 > self.buf.len() {
                    continue;
                }
                let a = bytes[s0 + 3] as u32;
                // src-over onto the accumulating buffer (which starts fully
                // transparent, so opaque sources simply replace).
                let inv = 255 - a;
                self.buf[d] =
                    (bytes[s0] as u32 + self.buf[d] as u32 * inv / 255).min(255) as u8;
                self.buf[d + 1] =
                    (bytes[s0 + 1] as u32 + self.buf[d + 1] as u32 * inv / 255).min(255) as u8;
                self.buf[d + 2] =
                    (bytes[s0 + 2] as u32 + self.buf[d + 2] as u32 * inv / 255).min(255) as u8;
                self.buf[d + 3] = (a + self.buf[d + 3] as u32 * inv / 255)
                    .min(255) as u8;
            }
        }
    }

    /// Alpha-composite the accumulated widget output onto `dst` (the wallpaper
    /// frame, BGRA, same dimensions).
    pub fn composite_pending(&mut self, dst: &mut [u8]) {
        let n = dst.len().min(self.buf.len());
        for d in (0..n).step_by(4) {
            let s = d;
            let a = self.buf[s + 3] as u32;
            if a == 0 {
                continue;
            }
            if a == 255 {
                dst[d] = self.buf[s];
                dst[d + 1] = self.buf[s + 1];
                dst[d + 2] = self.buf[s + 2];
            } else {
                let inv = 255 - a;
                dst[d] =
                    (self.buf[s] as u32 + dst[d] as u32 * inv / 255).min(255) as u8;
                dst[d + 1] =
                    (self.buf[s + 1] as u32 + dst[d + 1] as u32 * inv / 255).min(255) as u8;
                dst[d + 2] =
                    (self.buf[s + 2] as u32 + dst[d + 2] as u32 * inv / 255).min(255) as u8;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Rebuild
// ---------------------------------------------------------------------------

impl WidgetHost {
    /// Compile every `.rhai` script in the widgets directory.
    pub fn rebuild(size: (u32, u32), wallpaper_dir: &std::path::Path) -> WidgetHost {
        let dir = widgets_dir(wallpaper_dir);
        ensure_sample(&dir);

        let (cw, ch) = (size.0.max(1) as usize, size.1.max(1) as usize);
        // `size` is already in physical monitor pixels (the process is
        // per-monitor DPI aware), so do NOT multiply by dpi_scale here —
        // an oversized canvas would be downscaled at blit time, destroying
        // anti-aliasing. Script-facing DPI handling happens exclusively via
        // `pen_scale`, which normalizes the virtual canvas to this buffer.
        let pw = cw;
        let ph = ch;
        let mut host = WidgetHost {
            width: pw,
            height: ph,
            buf: vec![0u8; pw * ph * 4],
            items: Vec::new(),
            changed: false,
        };

        // Scale from the virtual canvas (VIRTUAL_W×VIRTUAL_H) to the
        // physical pixel buffer (pw×ph).  On 16:9 monitors the ratio is
        // identical for both axes so a single scale factor works.
        let pen_scale = pw as f64 / VIRTUAL_W as f64;

        let Ok(entries) = std::fs::read_dir(&dir) else {
            return host;
        };
        let mut scripts: Vec<_> = entries
            .flatten()
            .filter(|e| e.path().extension().and_then(|x| x.to_str()) == Some("rhai"))
            .map(|e| e.path())
            .collect();
        scripts.sort();

        for path in scripts {
            let name = path
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or("widget")
                .to_string();
            let res = (|| -> Result<(Engine, rhai::AST, Duration), String> {
                let src =
                    std::fs::read_to_string(&path).map_err(|e| format!("read: {e}"))?;
                let engine = make_engine();
                let ast = engine.compile(src).map_err(|e| format!("compile: {e}"))?;
                // Scripts may pick their own update rate with `fn fps()`.
                let fps = engine
                    .call_fn::<i64>(&mut Scope::new(), &ast, "fps", ())
                    .ok()
                    .filter(|f| (1..=120).contains(f))
                    .unwrap_or(WIDGET_FPS as i64);
                let interval =
                    Duration::from_millis((1000 / fps.max(1)).max(1) as u64);
                Ok((engine, ast, interval))
            })();
            match res {
                Ok((engine, ast, interval)) => {
                    let pen = Pen::new();
                    pen.set_scale(pen_scale);
                    pen.set_region(VIRTUAL_W, VIRTUAL_H);
                    host.items.push(ScriptItem {
                        name,
                        engine,
                        ast,
                        interval,
                        last_run: Instant::now() - Duration::from_secs(1),
                        pen,
                        hash: 0,
                        dirty: false,
                        last_bbox: None,
                    })
                }
                Err(e) => crate::logger::log(&format!("widget {name}: {e}")),
            }
        }
        host
    }
}

// ---------------------------------------------------------------------------
// Engine construction
// ---------------------------------------------------------------------------

fn make_engine() -> Engine {
    let mut engine = Engine::new();
    engine.register_type_with_name::<Pen>("Pen");

    engine.register_fn("clear", |p: &mut Pen| p.clear());
    engine.register_fn("set_opacity", |p: &mut Pen, a: f64| p.set_opacity(a as f32));
    engine.register_fn("state", |p: &mut Pen, key: &str| -> rhai::Dynamic { p.state(key) });
    engine.register_fn("set_state", |p: &mut Pen, key: &str, val: rhai::Dynamic| { p.set_state(key, val); });
    engine.register_fn("width", |p: &mut Pen| p.dims().0 as i64);
    engine.register_fn("height", |p: &mut Pen| p.dims().1 as i64);

    engine.register_fn(
        "fill_rect",
        |p: &mut Pen, x: i64, y: i64, w: i64, h: i64, color: &str| {
            p.fill_solid(x, y, w, h, parse_color(color));
        },
    );

    engine.register_fn(
        "line",
        |p: &mut Pen, x1: i64, y1: i64, x2: i64, y2: i64, t: i64, color: &str| {
            p.line(x1, y1, x2, y2, t, parse_color(color));
        },
    );

    engine.register_fn(
        "text",
        |p: &mut Pen, x: i64, y: i64, text: &str, px: i64, color: &str| {
            p.text(x, y, text, px, parse_color(color));
        },
    );

    engine.register_fn(
        "text",
        |p: &mut Pen, x: i64, y: i64, text: &str, px: i64, color: &str, font: &str| {
            p.text_font(x, y, text, px, parse_color(color), font);
        },
    );

    engine.register_fn(
        "text",
        |p: &mut Pen, x: i64, y: i64, text: &str, px: i64, color: &str, font: &str, spacing: i64| {
            p.text_font_spacing(x, y, text, px, parse_color(color), font, spacing);
        },
    );

    engine.register_fn("cpu", |p: &mut Pen| p.cpu_percent());
    engine.register_fn("ram", |p: &mut Pen| p.ram_percent());
    engine.register_fn("time", |p: &mut Pen, fmt: &str| p.time(fmt));
    engine.register_fn("date", |p: &mut Pen| p.date("%a, %b %e"));
    engine.register_fn("date", |p: &mut Pen, fmt: &str| p.date(fmt));

    // Drawing primitives
    engine.register_fn("arc", |p: &mut Pen, cx: i64, cy: i64, r: i64, start: f64, sweep: f64, t: i64, color: &str| {
        p.arc(cx, cy, r, start, sweep, t, parse_color(color));
    });
    engine.register_fn("circle", |p: &mut Pen, cx: i64, cy: i64, r: i64, t: i64, color: &str| {
        p.circle(cx, cy, r, t, parse_color(color));
    });
    engine.register_fn("fill_circle", |p: &mut Pen, cx: i64, cy: i64, r: i64, color: &str| {
        p.fill_circle(cx, cy, r, parse_color(color));
    });
    engine.register_fn("fill_round_rect", |p: &mut Pen, x: i64, y: i64, w: i64, h: i64, r: i64, color: &str| {
        p.fill_round_rect(x, y, w, h, r, parse_color(color));
    });

    // Battery
    engine.register_fn("battery", |p: &mut Pen| p.battery());
    engine.register_fn("charging", |p: &mut Pen| p.charging());

    // Bluetooth devices
    engine.register_fn("bt_count", |p: &mut Pen| p.bt_count());
    engine.register_fn("bt_level", |p: &mut Pen, i: i64| p.bt_level(i));
    engine.register_fn("bt_name", |p: &mut Pen, i: i64| p.bt_name(i));

    // Audio / Media
    engine.register_fn("audio_level", |p: &mut Pen, pos: f64| p.audio_level(pos));
    engine.register_fn("media_playing", |p: &mut Pen| p.media_playing());
    engine.register_fn("media_title", |p: &mut Pen| p.media_title());
    engine.register_fn("media_artist", |p: &mut Pen| p.media_artist());

    // HTTP / Process / Image
    engine.register_fn("http_get", |p: &mut Pen, url: &str| p.http_get_impl(url));
    engine.register_fn(
        "http_download",
        |p: &mut Pen, url: &str, path: &str| p.http_download_impl(url, path),
    );
    engine.register_fn("run", |p: &mut Pen, cmd: &str| p.exec_impl(cmd));
    engine.register_fn(
        "image",
        |p: &mut Pen, x: i64, y: i64, w: i64, h: i64, path: &str| {
            p.draw_image_impl(x, y, w, h, path);
        },
    );
    engine.register_fn(
        "svg",
        |p: &mut Pen, x: i64, y: i64, w: i64, h: i64, path: &str| {
            p.draw_svg_impl(x, y, w, h, path);
        },
    );
    engine.register_fn(
        "video",
        |p: &mut Pen, x: i64, y: i64, w: i64, h: i64, path: &str| {
            p.draw_video_impl(x, y, w, h, path);
        },
    );

    // GPU utilisation
    engine.register_fn("gpu", |p: &mut Pen| p.gpu_percent());

    // Regex
    engine.register_fn("regex_match", |pattern: &str, text: &str| Pen::regex_match(pattern, text));
    engine.register_fn("regex_find", |pattern: &str, text: &str| Pen::regex_find(pattern, text));
    engine.register_fn("regex_replace", |pattern: &str, text: &str, repl: &str| Pen::regex_replace(pattern, text, repl, false));
    engine.register_fn("regex_replace_all", |pattern: &str, text: &str, repl: &str| Pen::regex_replace(pattern, text, repl, true));
    engine.register_fn("regex_find_all", |pattern: &str, text: &str| {
        let matches: rhai::Array = regex::Regex::new(pattern)
            .ok()
            .map(|re| {
                re.find_iter(text)
                    .map(|m| rhai::Dynamic::from(m.as_str().to_string()))
                    .collect()
            })
            .unwrap_or_default();
        matches
    });

    // File I/O
    engine.register_fn("read_file", |path: &str| Pen::read_file(path));
    engine.register_fn("write_file", |path: &str, content: &str| Pen::write_file(path, content));
    engine.register_fn("file_exists", |path: &str| Pen::file_exists(path));

    // JSON
    engine.register_fn("json_parse", |text: &str| -> rhai::Dynamic {
        match serde_json::from_str::<serde_json::Value>(text) {
            Ok(v) => json_to_dynamic(&v),
            Err(_) => rhai::Dynamic::UNIT,
        }
    });
    engine.register_fn("json_stringify", |val: rhai::Dynamic| -> String {
        match dynamic_to_json(&val) {
            Ok(v) => v,
            Err(_) => "null".to_string(),
        }
    });
    engine.register_fn("json_get", |val: rhai::Dynamic, key: &str| -> rhai::Dynamic {
        if let Some(map) = val.clone().try_cast::<rhai::Map>() {
            map.get(key).cloned().unwrap_or(rhai::Dynamic::UNIT)
        } else if let Some(arr) = val.try_cast::<rhai::Array>() {
            if let Ok(idx) = key.parse::<usize>() {
                arr.get(idx).cloned().unwrap_or(rhai::Dynamic::UNIT)
            } else {
                rhai::Dynamic::UNIT
            }
        } else {
            rhai::Dynamic::UNIT
        }
    });

    engine
}

// ---------------------------------------------------------------------------
// JSON ↔ Rhai conversion helpers
// ---------------------------------------------------------------------------

fn json_to_dynamic(val: &serde_json::Value) -> rhai::Dynamic {
    use serde_json::Value;
    match val {
        Value::Null => rhai::Dynamic::UNIT,
        Value::Bool(b) => rhai::Dynamic::from(*b),
        Value::Number(n) => {
            if let Some(i) = n.as_i64() {
                rhai::Dynamic::from(i)
            } else if let Some(f) = n.as_f64() {
                rhai::Dynamic::from(f)
            } else {
                rhai::Dynamic::UNIT
            }
        }
        Value::String(s) => rhai::Dynamic::from(s.clone()),
        Value::Array(arr) => {
            let v: rhai::Array = arr.iter().map(json_to_dynamic).collect();
            rhai::Dynamic::from_array(v)
        }
        Value::Object(map) => {
            let mut m = rhai::Map::new();
            for (k, v) in map {
                m.insert(k.clone().into(), json_to_dynamic(v));
            }
            rhai::Dynamic::from_map(m)
        }
    }
}

fn dynamic_to_json(val: &rhai::Dynamic) -> Result<String, ()> {
    let v = dynamic_to_serde(val)?;
    serde_json::to_string(&v).map_err(|_| ())
}

fn dynamic_to_serde(val: &rhai::Dynamic) -> Result<serde_json::Value, ()> {
    if val.is::<()>() {
        Ok(serde_json::Value::Null)
    } else if val.is::<bool>() {
        Ok(serde_json::Value::Bool(val.clone().cast::<bool>()))
    } else if val.is::<i64>() {
        Ok(serde_json::json!(val.clone().cast::<i64>()))
    } else if val.is::<f64>() {
        Ok(serde_json::json!(val.clone().cast::<f64>()))
    } else if val.is::<String>() {
        Ok(serde_json::Value::String(val.clone().cast::<String>()))
    } else if val.is::<rhai::Array>() {
        let arr = val.clone().cast::<rhai::Array>();
        let v: Result<Vec<serde_json::Value>, ()> =
            arr.iter().map(dynamic_to_serde).collect();
        Ok(serde_json::Value::Array(v?))
    } else if val.is::<rhai::Map>() {
        let map = val.clone().cast::<rhai::Map>();
        let m: Result<serde_json::Map<String, serde_json::Value>, ()> = map
            .iter()
            .map(|(k, v)| Ok((k.to_string(), dynamic_to_serde(v)?)))
            .collect();
        Ok(serde_json::Value::Object(m?))
    } else {
        Ok(serde_json::Value::String(format!("{val}")))
    }
}
// ---------------------------------------------------------------------------

/// Cache of loaded custom fonts: maps canonical path → (family name, resource ID).
/// Fonts are loaded with FR_PRIVATE so they auto-remove on process exit.
fn font_cache() -> &'static Mutex<HashMap<std::path::PathBuf, (String, i32)>> {
    static CACHE: std::sync::OnceLock<Mutex<HashMap<std::path::PathBuf, (String, i32)>>> =
        std::sync::OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Resolve a font parameter: if it looks like a file path, load the TTF and
/// return its family name; otherwise return the string as-is.
fn resolve_font(font: &str) -> String {
    let p = std::path::Path::new(font);
    if !p.is_absolute() && !p.exists() {
        return font.to_string();
    }
    let Ok(canonical) = p.canonicalize() else {
        return font.to_string();
    };
    {
        let cache = font_cache().lock().unwrap();
        if let Some((family, _)) = cache.get(&canonical) {
            return family.clone();
        }
    }
    // Parse the family name from the TTF name table
    let family = std::fs::read(&canonical)
        .ok()
        .and_then(|data| {
            let face = ttf_parser::Face::parse(&data, 0).ok()?;
            face.names().into_iter().find_map(|n| {
                if n.name_id == ttf_parser::name_id::FAMILY {
                    n.to_string()
                } else {
                    None
                }
            })
        })
        .unwrap_or_else(|| {
            canonical
                .file_stem()
                .and_then(|s| s.to_str())
                .unwrap_or(font)
                .to_string()
        });
    unsafe {
        use windows::Win32::Graphics::Gdi::{AddFontResourceExW, FR_PRIVATE};
        let wide_path: Vec<u16> = canonical
            .to_string_lossy()
            .encode_utf16()
            .chain(std::iter::once(0))
            .collect();
        let id = AddFontResourceExW(
            windows::core::PCWSTR(wide_path.as_ptr()),
            FR_PRIVATE,
            None,
        );
        if id == 0 {
            return font.to_string();
        }
        let mut cache = font_cache().lock().unwrap();
        cache.insert(canonical, (family.clone(), id));
    }
    family
}

// ---------------------------------------------------------------------------
// Text rendering (GDI scratch bitmap -> coverage mask -> tinted blend)
// ---------------------------------------------------------------------------

fn draw_text_buffer(pen: &mut Pen, x: i64, y: i64, text: &str, px_i: i64, c: [u8; 4], font: &str, spacing: i64) {
    let (w, h) = pen.dims();
    if w == 0 || h == 0 || text.is_empty() {
        return;
    }
    let sc = { pen.inner.lock().unwrap().scale };
    let px = ((px_i as f64 * sc) as i64).clamp(6, 400) as i32;
    let family = resolve_font(font);

    // Supersample factor: render at 4x then downsample for smooth antialiasing.
    const S: usize = 4;

    // Scale logical coordinates to physical pixel space.
    let phys_x = (x as f64 * sc) as i64;
    let phys_y = (y as f64 * sc) as i64;
    let phys_spacing = (spacing as f64 * sc) as i64;
    let (pw, ph) = pen.phys();

    unsafe {
        use windows::Win32::Foundation::COLORREF;
        use windows::Win32::Foundation::SIZE;
        use windows::Win32::Graphics::Gdi::{
            CreateCompatibleDC, CreateDIBSection, CreateFontW, DeleteDC, DeleteObject, GetDC,
            GetTextExtentPoint32W, ReleaseDC, SelectObject, SetBkMode, SetTextColor, TextOutW,
            BITMAPINFO, BITMAPINFOHEADER, DIB_RGB_COLORS, TRANSPARENT,
        };

        let screen_dc = GetDC(None);
        let mem = CreateCompatibleDC(screen_dc);
        let _ = ReleaseDC(None, screen_dc);

        let s = S as i32;
        let spx = px * s;

        // Create font first so we can measure text width.
        let wide_font: Vec<u16> = family.encode_utf16().chain(std::iter::once(0)).collect();
        let hf = CreateFontW(-spx, 0, 0, 0, 600, 0, 0, 0, 0, 0, 0, 4, 0, windows::core::PCWSTR(wide_font.as_ptr()));
        let old_font = SelectObject(mem, hf);

        // Measure actual text width at supersampled size.
        let mut text_sz = SIZE::default();
        let wide_text: Vec<u16> = text.encode_utf16().collect();
        let _ = GetTextExtentPoint32W(mem, &wide_text, &mut text_sz);

        // Account for GDI overhang (italic/bold glyphs can extend past extent).
        use windows::Win32::Graphics::Gdi::{GetTextMetricsW, TEXTMETRICW};
        let mut tm = TEXTMETRICW::default();
        let _ = GetTextMetricsW(mem, &mut tm);
        let overhang = tm.tmOverhang;

        let spacing_w = if spacing != 0 {
            spacing.abs() as i32 * s * (text.chars().count() as i32 - 1).max(0)
        } else {
            0
        };
        let pad = (s * 8).max(overhang * s + s * 4);
        let est_w = text_sz.cx + spacing_w + pad;
        let est_h = text_sz.cy + pad;
        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = est_w;
        bmi.bmiHeader.biHeight = -est_h;
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = 0;
        let mut bits: *mut core::ffi::c_void = std::ptr::null_mut();
        let bmp = match CreateDIBSection(mem, &bmi, DIB_RGB_COLORS, &mut bits, None, 0) {
            Ok(b) => b,
            Err(_) => {
                let _ = DeleteDC(mem);
                return;
            }
        };
        let old_bmp = SelectObject(mem, bmp);
        let _ = SetBkMode(mem, TRANSPARENT);
        let _ = SetTextColor(mem, COLORREF(0x00FF_FFFF)); // white glyphs

        if spacing == 0 {
            let wide: Vec<u16> = text.encode_utf16().collect();
            let _ = TextOutW(mem, s, s, &wide);
        } else {
            let mut cx: i32 = s;
            let mut buf = [0u16; 2];
            for ch in text.chars() {
                let len = ch.encode_utf16(&mut buf);
                let _ = TextOutW(mem, cx, s, len);
                let mut sz = SIZE::default();
                let _ = GetTextExtentPoint32W(mem, len, &mut sz);
                cx += sz.cx + phys_spacing as i32 * s;
            }
        }

        // Downsample the supersampled coverage mask by averaging S×S blocks.
        let stride = est_w as usize * 4;
        let data = std::slice::from_raw_parts(bits.cast::<u8>(), stride * est_h as usize);
        // Collect all non-zero pixels first, then blend in a single lock.
        let mut pixels: Vec<(usize, usize, u8)> = Vec::new();
        for gy in 0..est_h as usize / S {
            let dy = phys_y as usize + gy;
            if dy >= ph {
                break;
            }
            for gx in 0..est_w as usize / S {
                let dx = phys_x as usize + gx;
                if dx >= pw {
                    break;
                }
                let mut sum: u32 = 0;
                for sy in 0..S as usize {
                    for sx in 0..S as usize {
                        let so = (gy * S as usize + sy) * stride + (gx * S as usize + sx) * 4;
                        let cov = data[so].max(data[so + 1]).max(data[so + 2]) as u32;
                        sum += cov;
                    }
                }
                let cov = (sum / (S * S) as u32) as u32;
                if cov == 0 {
                    continue;
                }
                let a = (cov * c[3] as u32) / 255;
                pixels.push((dx, dy, a as u8));
            }
        }
        if !pixels.is_empty() {
            let mut s = pen.inner.lock().unwrap();
            let opa = s.opacity;
            for (dx, dy, a) in &pixels {
                let a = if opa < 1.0 { (*a as f32 * opa) as u8 } else { *a };
                if a == 0 { continue; }
                let o = (*dy * pw + *dx) * 4;
                if o + 3 >= s.pixels.len() { continue; }
                let inv = 255 - a as u32;
                let pr = ((c[0] as u32 * a as u32) / 255) as u8;
                let pg = ((c[1] as u32 * a as u32) / 255) as u8;
                let pb = ((c[2] as u32 * a as u32) / 255) as u8;
                s.pixels[o] = (pb as u32 + s.pixels[o] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 1] = (pg as u32 + s.pixels[o + 1] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 2] = (pr as u32 + s.pixels[o + 2] as u32 * inv / 255).min(255) as u8;
                s.pixels[o + 3] = (a as u32 + s.pixels[o + 3] as u32 * inv / 255).min(255) as u8;
            }
        }

        SelectObject(mem, old_font);
        let _ = DeleteObject(hf);
        SelectObject(mem, old_bmp);
        let _ = DeleteObject(bmp);
        let _ = DeleteDC(mem);
    }
}

// ---------------------------------------------------------------------------
// HTTP / Process / Image infrastructure
// ---------------------------------------------------------------------------

/// Default HTTP response cache TTL.
const HTTP_TTL: Duration = Duration::from_secs(60);

type HttpCache = HashMap<String, (Instant, String)>;
type ImageCache = HashMap<String, (u32, u32, Arc<Vec<u8>>)>;

fn http_cache() -> &'static Mutex<HttpCache> {
    static CACHE: OnceLock<Mutex<HttpCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

fn image_cache() -> &'static Mutex<ImageCache> {
    static CACHE: OnceLock<Mutex<ImageCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

type VideoPlayers = HashMap<PathBuf, Arc<crate::video::VideoPlayer>>;

/// One background decoder per video path used by widgets. Players keep
/// looping their source forever; frames are fetched at draw time.
fn video_players() -> &'static Mutex<VideoPlayers> {
    static PLAYERS: OnceLock<Mutex<VideoPlayers>> = OnceLock::new();
    PLAYERS.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Get (or lazily open) the background player for a video file.
fn video_player_for(
    path: &Path,
    target: Option<(u32, u32)>,
) -> Option<Arc<crate::video::VideoPlayer>> {
    let mut players = video_players().lock().unwrap();
    if let Some(p) = players.get(path) {
        return Some(p.clone());
    }
    let data = std::fs::read(path).ok()?;
    let data = Arc::new(data);
    // GPU decode first, software fallback — same policy as wallpapers.
    let player = crate::video::VideoPlayer::open(data.clone(), target, true)
        .or_else(|_| crate::video::VideoPlayer::open(data, target, false))
        .ok()?;
    let arc = Arc::new(player);
    players.insert(path.to_path_buf(), arc.clone());
    Some(arc)
}

type SvgCache = HashMap<(PathBuf, u32, u32), Arc<Vec<u8>>>;

/// Render an SVG to straight RGBA at an exact size, cached per path+size.
fn render_svg_cached(path: &Path, tw: u32, th: u32) -> Option<Arc<Vec<u8>>> {
    {
        let cache = svg_cache().lock().ok()?;
        if let Some(rgba) = cache.get(&(path.to_path_buf(), tw, th)) {
            return Some(rgba.clone());
        }
    }
    let data = std::fs::read(path).ok()?;
    let rgba = render_svg(&data, tw, th)?;
    let arc = Arc::new(rgba);
    if let Ok(mut cache) = svg_cache().lock() {
        cache.insert((path.to_path_buf(), tw, th), arc.clone());
    }
    Some(arc)
}

fn svg_cache() -> &'static Mutex<SvgCache> {
    static CACHE: OnceLock<Mutex<SvgCache>> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

pub fn clear_svg_cache() {
    if let Ok(mut cache) = svg_cache().lock() {
        cache.clear();
    }
}

/// Rasterise SVG bytes into straight-alpha RGBA pixels of size `(tw x th)`.
fn render_svg(data: &[u8], tw: u32, th: u32) -> Option<Vec<u8>> {
    use resvg::tiny_skia;
    use resvg::usvg;

    let opts = usvg::Options::default();
    let tree = usvg::Tree::from_data(data, &opts).ok()?;
    let mut pixmap = tiny_skia::Pixmap::new(tw, th)?;
    pixmap.fill(tiny_skia::Color::TRANSPARENT);

    // Scale from the SVG's intrinsic size to the requested output size.
    let size = tree.size();
    let sx = tw as f32 / size.width().max(1.0);
    let sy = th as f32 / size.height().max(1.0);
    {
        let mut view = pixmap.as_mut();
        resvg::render(&tree, tiny_skia::Transform::from_scale(sx, sy), &mut view);
    }

    // tiny-skia stores premultiplied RGBA; convert to straight alpha for the
    // shared blend helper.
    let raw = pixmap.take();
    let mut out = Vec::with_capacity(raw.len());
    for px in raw.chunks_exact(4) {
        let a = px[3] as u32;
        if a == 0 || a == 255 {
            out.extend_from_slice(px);
        } else {
            out.push((px[0] as u32 * 255 / a) as u8);
            out.push((px[1] as u32 * 255 / a) as u8);
            out.push((px[2] as u32 * 255 / a) as u8);
            out.push(a as u8);
        }
    }
    Some(out)
}

fn resolve_widget_path(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() && p.exists() {
        return p.to_path_buf();
    }
    let base = crate::storage::wallpaper_dir();
    let candidate = base.join(p);
    if candidate.exists() {
        return candidate;
    }
    let widgets = base.join("widgets").join(p);
    if widgets.exists() {
        return widgets;
    }
    candidate
}

/// Nearest-neighbour resize of raw RGBA pixels.
pub(crate) fn resize_rgba(
    src: &[u8],
    sw: usize,
    sh: usize,
    tw: usize,
    th: usize,
) -> Vec<u8> {
    let mut out = vec![0u8; tw * th * 4];
    for row in 0..th {
        let sy = row * sh / th;
        for col in 0..tw {
            let sx = col * sw / tw;
            let so = (sy * sw + sx) * 4;
            let do_ = (row * tw + col) * 4;
            if so + 4 <= src.len() && do_ + 4 <= out.len() {
                out[do_..do_ + 4].copy_from_slice(&src[so..so + 4]);
            }
        }
    }
    out
}

/// HTTP GET with automatic 60-second caching per URL.
pub(crate) fn http_get_cached(url: &str) -> String {
    // Check cache first
    {
        let cache = http_cache().lock().unwrap();
        if let Some((fetched, body)) = cache.get(url) {
            if fetched.elapsed() < HTTP_TTL {
                return body.clone();
            }
        }
    }

    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(15))
        .build();

    match agent.get(url).call() {
        Ok(resp) => {
            let body = resp.into_string().unwrap_or_default();
            http_cache()
                .lock()
                .unwrap()
                .insert(url.to_string(), (Instant::now(), body.clone()));
            body
        }
        Err(e) => {
            crate::logger::log(&format!("http_get {url}: {e}"));
            String::new()
        }
    }
}

/// Download binary content to a local file (for images etc).
fn http_download_to(url: &str, save_path: &Path) -> Result<(), String> {
    let agent = ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(5))
        .timeout(Duration::from_secs(30))
        .build();
    let resp = agent
        .get(url)
        .call()
        .map_err(|e| format!("download {url}: {e}"))?;
    let mut reader = resp.into_reader();
    let mut buf = Vec::new();
    std::io::Read::read_to_end(&mut reader, &mut buf)
        .map_err(|e| format!("read body failed: {e}"))?;
    std::fs::write(save_path, &buf).map_err(|e| format!("write failed: {e}"))
}

/// Run a shell command and return trimmed stdout.
fn exec_command(cmd: &str) -> String {
    std::process::Command::new("cmd")
        .args(["/C", cmd])
        .output()
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Load + resize an image from disk (cached by resolved path).
fn load_image_cached(resolved: &Path) -> Option<(u32, u32, Arc<Vec<u8>>)> {
    {
        let cache = image_cache().lock().ok()?;
        if let Some((w, h, data)) = cache.get(&resolved.to_string_lossy().to_string()) {
            return Some((*w, *h, data.clone()));
        }
    }
    let img = image::open(resolved).ok()?;
    let rgba = img.to_rgba8();
    let (w, h) = (img.width(), img.height());
    let data = Arc::new(rgba.into_raw());
    image_cache()
        .lock()
        .unwrap()
        .insert(resolved.to_string_lossy().to_string(), (w, h, data.clone()));
    Some((w, h, data))
}

/// Blend an RGBA pixel buffer onto the canvas at `(x, y)` with dimensions `(w, h)`.
/// Coordinates are in logical space; the function scales to physical pixels.
fn blend_rgba_at(pen: &Pen, x: i64, y: i64, w: usize, h: usize, rgba: &[u8]) {
    let mut s = pen.inner.lock().unwrap();
    let sc = s.scale;
    let pw = (s.w as f64 * sc).ceil() as i64;
    let ph = (s.h as f64 * sc).ceil() as i64;
    let sx = (x as f64 * sc) as i64;
    let sy = (y as f64 * sc) as i64;
    for row in 0..h {
        let dy = sy + row as i64;
        if dy < 0 || dy >= ph {
            continue;
        }
        let dy = dy as usize;
        for col in 0..w {
            let dx = sx + col as i64;
            if dx < 0 || dx >= pw {
                continue;
            }
            let dx = dx as usize;
            let so = (row * w + col) * 4;
            if so + 4 > rgba.len() {
                break;
            }
            let a = rgba[so + 3] as u32;
            if a == 0 {
                continue;
            }
            let d = (dy * pw as usize + dx) * 4;
            let inv = 255 - a;
            let src_r = ((rgba[so + 2] as u32 * a) / 255) as u32;
            let src_g = ((rgba[so + 1] as u32 * a) / 255) as u32;
            let src_b = ((rgba[so] as u32 * a) / 255) as u32;
            s.pixels[d] = (src_r + s.pixels[d] as u32 * inv / 255).min(255) as u8;
            s.pixels[d + 1] = (src_g + s.pixels[d + 1] as u32 * inv / 255).min(255) as u8;
            s.pixels[d + 2] = (src_b + s.pixels[d + 2] as u32 * inv / 255).min(255) as u8;
            s.pixels[d + 3] = (a + s.pixels[d + 3] as u32 * inv / 255).min(255) as u8;
        }
    }
}

/// Format a `SYSTEMTIME` using `strftime`-style specifiers:
/// `%H %M %S %Y %m %d %e %p %a %A %b %B`.
fn format_datetime(st: windows::Win32::Foundation::SYSTEMTIME, fmt: &str) -> String {
    const WEEKDAYS_SHORT: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
    const WEEKDAYS_LONG: [&str; 7] = [
        "Sunday", "Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday",
    ];
    const MONTHS_SHORT: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    const MONTHS_LONG: [&str; 12] = [
        "January", "February", "March", "April", "May", "June",
        "July", "August", "September", "October", "November", "December",
    ];

    let mut out = String::new();
    let chars: Vec<char> = fmt.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '%' && i + 1 < chars.len() {
            let piece = match chars[i + 1] {
                'H' => format!("{:02}", st.wHour),
                'M' => format!("{:02}", st.wMinute),
                'S' => format!("{:02}", st.wSecond),
                'Y' => format!("{}", st.wYear),
                'm' => format!("{:02}", st.wMonth),
                'd' => format!("{:02}", st.wDay),
                'e' => format!("{:2}", st.wDay),
                'p' => if st.wHour < 12 { "AM".into() } else { "PM".into() },
                'a' => WEEKDAYS_SHORT[(st.wDayOfWeek as usize) % 7].into(),
                'A' => WEEKDAYS_LONG[(st.wDayOfWeek as usize) % 7].into(),
                'b' | 'h' => MONTHS_SHORT[(st.wMonth as usize).saturating_sub(1) % 12].into(),
                'B' => MONTHS_LONG[(st.wMonth as usize).saturating_sub(1) % 12].into(),
                other => format!("%{other}"),
            };
            out.push_str(&piece);
            i += 2;
        } else {
            out.push(chars[i]);
            i += 1;
        }
    }
    out
}

fn parse_color(hex: &str) -> [u8; 4] {
    let s = hex.trim().trim_start_matches('#');
    let val = |r: &str| u8::from_str_radix(r, 16).unwrap_or(255);
    match s.len() {
        8 => [
            val(&s[0..2]),
            val(&s[2..4]),
            val(&s[4..6]),
            val(&s[6..8]),
        ],
        6 => [val(&s[0..2]), val(&s[2..4]), val(&s[4..6]), 255],
        3 => {
            let v = |c: &str| u8::from_str_radix(&format!("{c}{c}"), 16).unwrap_or(255);
            [v(&s[0..1]), v(&s[1..2]), v(&s[2..3]), 255]
        }
        _ => [255, 255, 255, 255],
    }
}

/// Convert RGBA colour to premultiplied BGRA for the canvas buffer.
fn premul_bgra(c: [u8; 4]) -> [u8; 4] {
    let a = c[3] as u16;
    [
        ((c[2] as u16 * a) / 255) as u8, // B premultiplied
        ((c[1] as u16 * a) / 255) as u8, // G premultiplied
        ((c[0] as u16 * a) / 255) as u8, // R premultiplied
        c[3],
    ]
}

static CPU_LAST: Mutex<Option<(u64, u64, u64)>> = Mutex::new(None);

fn cpu_usage() -> i64 {
    unsafe {
        use windows::Win32::Foundation::FILETIME;
        use windows::Win32::System::Threading::GetSystemTimes;
        let mut idle = FILETIME::default();
        let mut kernel = FILETIME::default();
        let mut user = FILETIME::default();
        if GetSystemTimes(Some(&mut idle), Some(&mut kernel), Some(&mut user)).is_err() {
            return 0;
        }
        let ft = |t: &FILETIME| ((t.dwHighDateTime as u64) << 32) | t.dwLowDateTime as u64;
        let cur = (ft(&idle), ft(&kernel), ft(&user));
        let mut prev = CPU_LAST.lock().unwrap();
        let usage = match *prev {
            Some((pi, pk, pu)) => {
                let total = (cur.1.wrapping_sub(pk)).wrapping_add(cur.2.wrapping_sub(pu));
                let busy = total.saturating_sub(cur.0.wrapping_sub(pi));
                if total == 0 {
                    0
                } else {
                    (busy * 100 / total).min(100)
                }
            }
            None => 0,
        };
        *prev = Some(cur);
        usage as i64
    }
}

fn fnv_hash(bytes: &[u8]) -> u64 {
    // Sampled FNV-1a: striding keeps large canvases cheap while still
    // catching any real content change with near-certainty.
    const STRIDE: usize = 97;
    let mut hash: u64 = 0xcbf2_9ce4_8422_2325;
    let mut i = 0;
    while i < bytes.len() {
        hash ^= bytes[i] as u64;
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
        i += STRIDE;
    }
    hash ^ (bytes.len() as u64)
}
