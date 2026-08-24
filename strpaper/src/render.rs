//! Wallpaper decoding and painting.
//!
//! Supported still/animated formats are fully decoded with the `image` crate.
//! Every decoded frame is stored as a 32-bit BGRA raster (top-down rows of
//! bytes in B,G,R,A order) ready to be handed straight to GDI.
//!
//! Painting happens by drawing the raster into the wallpaper child window's
//! device context, once per monitor, using a "cover" fit (crop to fill, keep
//! aspect ratio, centre crop). Coordinates are relative to the wallpaper
//! window origin, which is passed as `origin`.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use image::codecs::gif::GifDecoder;
use image::AnimationDecoder;

use crate::desktop::Monitor;

use windows::Win32::Foundation::RECT;
use windows::Win32::Graphics::Gdi::{
    FillRect, GetStockObject, SetStretchBltMode, StretchDIBits, BITMAPINFO, BITMAPINFOHEADER,
    DIB_USAGE, GET_STOCK_OBJECT_FLAGS, HBRUSH, HDC, ROP_CODE, STRETCH_BLT_MODE,
};

// GDI constants (kept local to avoid name-resolution churn across versions).
const DIB_RGB_COLORS: DIB_USAGE = DIB_USAGE(0);
const SRCCOPY: ROP_CODE = ROP_CODE(0x00CC_0020);
const COLORONCOLOR: STRETCH_BLT_MODE = STRETCH_BLT_MODE(0x0003);
const BLACK_BRUSH: GET_STOCK_OBJECT_FLAGS = GET_STOCK_OBJECT_FLAGS(4);
const BI_RGB: u32 = 0;

/// A decoded, quantised frame in BGRA byte order (top-down rows).
#[derive(Clone)]
pub struct Raster {
    pub width: usize,
    pub height: usize,
    /// `width * height * 4` bytes arranged B,G,R,A.
    pub bgra: Vec<u8>,
}

impl Raster {
    fn from_rgba(rgba: &[u8], width: usize, height: usize) -> Raster {
        let mut bgra = Vec::with_capacity(rgba.len());
        for px in rgba.chunks_exact(4) {
            bgra.push(px[2]); // B
            bgra.push(px[1]); // G
            bgra.push(px[0]); // R
            bgra.push(px[3]); // A
        }
        Raster {
            width,
            height,
            bgra,
        }
    }
}

/// A single frame of an animated wallpaper (e.g. GIF).
#[derive(Clone)]
pub struct AnimatedFrame {
    pub raster: Arc<Raster>,
    pub delay: Duration,
}

/// A decoded animated wallpaper.
pub struct Animated {
    pub frames: Vec<AnimatedFrame>,
    pub total: Duration,
}

/// A fully-decoded wallpaper source ready to be painted.
pub enum Wallpaper {
    Still(Arc<Raster>),
    Animated(Animated),
    Video(crate::video::VideoPlayer),
}

/// Whether this source has continuous motion that must be re-painted at a
/// regular cadence.
pub fn needs_ticks(w: &Wallpaper) -> bool {
    matches!(w, Wallpaper::Animated(_) | Wallpaper::Video(_))
}

/// Select the raster that should be on screen at `elapsed` since playback
/// started. The returned `Arc` keeps the frame buffer alive while it is being
/// painted, even if a new frame replaces it concurrently. Returns `None` when
/// nothing is available to paint.
pub fn frame_at(w: &mut Wallpaper, elapsed: Duration) -> Option<Arc<Raster>> {
    match w {
        Wallpaper::Still(r) => Some(r.clone()),
        Wallpaper::Animated(a) => gif_frame_at(a, elapsed).cloned(),
        Wallpaper::Video(v) => v.frame_at(elapsed),
    }
}

/// Scale a raster to exactly `(tw x th)` using a centre-crop "cover" fit.
/// `fast` selects nearest-neighbour (GIF frames); otherwise triangle filtering
/// is used (still images).
pub fn fit_cover(raster: &Raster, tw: usize, th: usize, fast: bool) -> Raster {
    use image::imageops::FilterType;
    let filter = if fast { FilterType::Nearest } else { FilterType::Triangle };

    // BGRA -> RGBA for the image crate.
    let mut rgba = Vec::with_capacity(raster.bgra.len());
    for px in raster.bgra.chunks_exact(4) {
        rgba.push(px[2]);
        rgba.push(px[1]);
        rgba.push(px[0]);
        rgba.push(px[3]);
    }
    let Some(ib) = image::RgbaImage::from_raw(raster.width as u32, raster.height as u32, rgba)
    else {
        return raster.clone();
    };

    // Centre-crop to the target aspect ratio, then resize.
    let src_ar = raster.width as f64 / raster.height.max(1) as f64;
    let dst_ar = tw as f64 / th.max(1) as f64;
    let cropped = if src_ar > dst_ar {
        let cw = ((raster.height as f64) * dst_ar).floor() as u32;
        let x = raster.width.saturating_sub(cw as usize) as u32 / 2;
        image::imageops::crop_imm(&ib, x, 0, cw, raster.height as u32)
    } else {
        let ch = ((raster.width as f64) / dst_ar).floor() as u32;
        let y = raster.height.saturating_sub(ch as usize) as u32 / 2;
        image::imageops::crop_imm(&ib, 0, y, raster.width as u32, ch)
    };
    let resized = image::imageops::resize(&cropped.to_image(), tw as u32, th as u32, filter);

    // RGBA -> BGRA.
    let mut bgra = resized.into_raw();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    Raster {
        width: tw,
        height: th,
        bgra,
    }
}

/// Decode a still image file into a raster.
pub fn decode_still(path: &Path) -> Result<Raster, String> {    let img = image::open(path).map_err(|e| format!("image open failed: {e}"))?;
    let rgba = img.to_rgba8();
    let (w, h) = (rgba.width() as usize, rgba.height() as usize);
    Ok(Raster::from_rgba(&rgba.into_raw(), w, h))
}

/// Decode an animated image (GIF) into its frames.
pub fn decode_animated(path: &Path) -> Result<Animated, String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open failed: {e}"))?;
    let decoder = GifDecoder::new(std::io::BufReader::new(file))
        .map_err(|e| format!("gif decode init failed: {e}"))?;
    let mut frames = Vec::new();
    for frame in decoder.into_frames() {
        let frame = frame.map_err(|e| format!("gif frame decode failed: {e}"))?;
        let delay = frame.delay();
        let ms = delay.numer_denom_ms();
        let delay = if ms.0 == 0 {
            Duration::from_millis(100)
        } else {
            Duration::from_millis((ms.0 as u64) * 1000 / (ms.1.max(1) as u64))
        };
        let img = frame.buffer().to_owned();
        let (w, h) = (img.width() as usize, img.height() as usize);
        frames.push(AnimatedFrame {
            raster: Arc::new(Raster::from_rgba(&img.into_raw(), w, h)),
            delay,
        });
    }
    if frames.is_empty() {
        return Err("gif contains no frames".into());
    }
    let total = frames.iter().fold(Duration::ZERO, |a, f| a + f.delay);
    Ok(Animated { frames, total })
}

/// Select the GIF frame whose time window contains `elapsed`, looping around
/// the animation timeline.
fn gif_frame_at(a: &Animated, elapsed: Duration) -> Option<&Arc<Raster>> {
    if a.frames.is_empty() {
        return None;
    }
    let total = if a.total.is_zero() {
        Duration::from_millis(100)
    } else {
        a.total
    };
    let t = elapsed.as_millis() as u64 % total.as_millis().max(1) as u64;
    let mut elapsed_so_far = 0u64;
    for frame in &a.frames {
        elapsed_so_far += frame.delay.as_millis() as u64;
        if t < elapsed_so_far {
            return Some(&frame.raster);
        }
    }
    a.frames.last().map(|f| &f.raster)
}

/// Build the GDI bitmap info describing a BGRA raster.
fn bitmap_info_for(w: i32, h: i32) -> BITMAPINFO {
    let header = BITMAPINFOHEADER {
        biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
        biWidth: w,
        biHeight: -h, // negative = top-down rows
        biPlanes: 1,
        biBitCount: 32,
        biCompression: BI_RGB,
        biSizeImage: (w * h * 4) as u32,
        ..Default::default()
    };
    BITMAPINFO {
        bmiHeader: header,
        bmiColors: Default::default(),
    }
}

/// Draw `raster` into `hdc` for every monitor, using a cover fit. `monitors`
/// are in virtual-desktop coordinates; `origin` is the wallpaper window origin
/// that the drawing is offset by.
pub fn paint_frame(hdc: HDC, monitors: &[Monitor], origin: (i32, i32), raster: &Raster) {
    unsafe {
        let _ = SetStretchBltMode(hdc, COLORONCOLOR);
        let (sw, sh) = (raster.width as i32, raster.height as i32);
        if sw <= 0 || sh <= 0 {
            return;
        }
        let bmi = bitmap_info_for(sw, sh);
        for mon in monitors {
            if let Some((dst, src)) = cover_rects(sw, sh, *mon) {
                let _ = StretchDIBits(
                    hdc,
                    dst.0 - origin.0,
                    dst.1 - origin.1,
                    dst.2,
                    dst.3,
                    src.0,
                    src.1,
                    src.2,
                    src.3,
                    Some(raster.bgra.as_ptr() as *const _),
                    &bmi,
                    DIB_RGB_COLORS,
                    SRCCOPY,
                );
            }
        }
    }
}

/// Overwrite every monitor region with opaque black (used when the wallpaper
/// is removed).
pub fn paint_clear(hdc: HDC, monitors: &[Monitor], origin: (i32, i32)) {
    unsafe {
        let brush = HBRUSH(GetStockObject(BLACK_BRUSH).0);
        for mon in monitors {
            let rect = RECT {
                left: mon.left - origin.0,
                top: mon.top - origin.1,
                right: mon.left - origin.0 + mon.width,
                bottom: mon.top - origin.1 + mon.height,
            };
            let _ = FillRect(hdc, &rect, brush);
        }
    }
}

/// Compute the destination rectangle for a cover fit of an image of size
/// `(sw, sh)` into `mon`, together with the source crop rectangle.
fn cover_rects(sw: i32, sh: i32, mon: Monitor) -> Option<((i32, i32, i32, i32), (i32, i32, i32, i32))> {
    let (mw, mh) = (mon.width, mon.height);
    if sw <= 0 || sh <= 0 || mw <= 0 || mh <= 0 {
        return None;
    }
    let (crop_x, crop_y, crop_w, crop_h) = centre_crop(sw, sh, mw, mh);
    let dst = (mon.left, mon.top, mw, mh);
    let src = (crop_x, crop_y, crop_w, crop_h);
    Some((dst, src))
}

/// Centre-crop a `(sw, sh)` source so that its aspect ratio matches `(dw, dh)`.
fn centre_crop(sw: i32, sh: i32, dw: i32, dh: i32) -> (i32, i32, i32, i32) {
    let target_ratio = dw as f64 / dh.max(1) as f64;
    let src_ratio = sw as f64 / sh.max(1) as f64;
    if src_ratio > target_ratio {
        // Source is wider than target: crop the sides.
        let crop_w = (sh as f64 * target_ratio).round() as i32;
        let crop_w = crop_w.min(sw);
        ((sw - crop_w) / 2, 0, crop_w, sh)
    } else {
        // Source is taller than target: crop the top and bottom.
        let crop_h = (sw as f64 / target_ratio).round() as i32;
        let crop_h = crop_h.min(sh);
        (0, (sh - crop_h) / 2, sw, crop_h)
    }
}
