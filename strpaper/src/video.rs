//! Media Foundation video playback for MP4 / WebM wallpapers.
//!
//! Decoding is performed on a dedicated background thread so the UI thread is
//! never blocked (no frozen wallpaper / busy cursor). Each decoded frame is
//! published as an [`Arc<Raster>]` that the UI thread blits from. If the
//! source cannot be decoded, the player reports a `Failed` status and the
//! caller hides the wallpaper window, keeping the application responsive.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use windows::core::{GUID, Interface};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFMediaType, IMFSample, IMFSourceReader, MFCreateMediaType,
    MFCreateMFByteStreamOnStream, MFCreateMemoryBuffer, MFCreateSample,
    MFCreateSinkWriterFromURL, MFCreateSourceReaderFromByteStream, MFShutdown, MFStartup,
    MFMediaType_Video, MF_MT_AVG_BITRATE, MF_MT_FRAME_RATE, MF_MT_FRAME_SIZE, MF_MT_MAJOR_TYPE,
    MF_MT_SUBTYPE,     MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR, MFVideoFormat_H264,
    MFVideoFormat_NV12, MFVideoFormat_RGB24, MFVideoFormat_RGB32, MFVideoFormat_YUY2, MF_VERSION,
    msoBegin,
};
use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
use windows::Win32::UI::Shell::SHCreateMemStream;

use crate::render::Raster;

/// Default frame duration used when the media provides no timing.
const DEFAULT_FRAME_DUR: Duration = Duration::from_millis(33);
/// Media Foundation presents sample durations in 100ns ticks.
const TICKS_TO_NANOS: u64 = 100;
/// Short sleep used to avoid a hot loop between readable samples.
const THROTTLE: Duration = Duration::from_millis(4);

/// Initialize Media Foundation once for the process.
pub fn startup() -> Result<(), String> {
    unsafe { MFStartup(MF_VERSION, 0) }.map_err(|e| format!("MFStartup failed: {e}"))
}

/// Shut down Media Foundation.
pub fn shutdown() {
    let _ = unsafe { MFShutdown() };
}

enum Status {
    Ok,
    Failed(String),
}

struct Shared {
    current: Arc<Raster>,
    status: Status,
    version: u64,
}

/// A live video wallpaper player backed by a background decode thread.
pub struct VideoPlayer {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    started: Instant,
    hw: bool,
    /// Shared pause flag: while set, the decode loop idles (no GPU/CPU work).
    paused: Arc<AtomicBool>,
}

impl VideoPlayer {
    /// Decode dimensions of an in-memory media file without starting playback.
    pub fn probe_size(data: &[u8]) -> Option<(u32, u32)> {
        let _com = ComGuard;
        let (_, _, w, h, _) = unsafe { init_reader(data, None) }.ok()?;
        Some((w, h))
    }

    /// Re-encode an in-memory video into H.264 at `out_w x out_h`, entirely in
    /// memory: frames come from the normal decode pipeline and go straight
    /// into a Media Foundation Sink Writer backed by a memory byte stream.
    /// Blocking — run it on a background thread.
    pub fn transcode(
        data: Arc<Vec<u8>>,
        out_w: u32,
        out_h: u32,
    ) -> Result<Vec<u8>, String> {
        let _com = ComGuard;
        let (out_w, out_h) = (out_w | 1, out_h | 1); // H.264 needs even sizes
        let mut reader = Reader::new(&data, Some((out_w, out_h)), None)?;

        unsafe {
            // Writable memory stream that receives the encoded file.
            let stream = SHCreateMemStream(None).ok_or("create memory stream failed")?;
            let bytestream = MFCreateMFByteStreamOnStream(&stream)
                .map_err(|e| format!("create byte stream failed: {e}"))?;
            let sink = MFCreateSinkWriterFromURL(
                windows::core::PCWSTR::null(),
                &bytestream,
                None,
            )
            .map_err(|e| format!("create sink writer failed: {e}"))?;

            let out_type = transcode_output_type(out_w as u32, out_h as u32)?;
            let idx = sink.AddStream(&out_type).map_err(|e| format!("add stream failed: {e}"))?;

            let in_type = transcode_input_type(out_w as u32, out_h as u32)?;
            sink.SetInputMediaType(idx, &in_type, None)
                .map_err(|e| format!("set input type failed: {e}"))?;
            sink.BeginWriting().map_err(|e| format!("begin writing failed: {e}"))?;

            let mut time100ns: i64 = 0;
            loop {
                let step = match reader.next() {
                    Next::Frame(raster) => {
                        // 100ns ticks, matching Media Foundation timing.
                        let dur100 = (reader.frame_dur.as_nanos() / 100).max(1) as i64;
                        write_bgra_sample(
                            &sink,
                            idx,
                            &raster.bgra,
                            time100ns,
                            dur100,
                        )?;
                        time100ns += dur100;
                        None
                    }
                    Next::EndOfStream => Some(()),
                    Next::Again => {
                        thread::sleep(THROTTLE);
                        None
                    }
                    Next::Error(e) => return Err(format!("transcode decode failed: {e}")),
                };
                if step.is_some() {
                    break;
                }
            }

            sink.Finalize().map_err(|e| format!("finalize failed: {e}"))?;

            // Pull the finished file back out of the memory stream.
            bytestream.Seek(msoBegin, 0, 0).map_err(|e| format!("seek failed: {e}"))?;
            let mut out = Vec::new();
            let mut chunk = vec![0u8; 256 * 1024];
            loop {
                let mut read = 0u32;
                bytestream
                    .Read(&mut chunk, &mut read)
                    .map_err(|e| format!("stream read failed: {e}"))?;
                if read == 0 {
                    break;
                }
                out.extend_from_slice(&chunk[..read as usize]);
            }
            if out.is_empty() {
                return Err("transcode produced no data".into());
            }
            Ok(out)
        }
    }

    /// Start decoding an in-memory media file. Returns immediately; frames
    /// are produced on a background thread. The application is never blocked.
    ///
    /// `target` is the output size to decode into (typically the monitor
    /// resolution) — decoding straight to screen size avoids converting and
    /// blitting far more pixels than will ever be shown.
    pub fn open(
        data: Arc<Vec<u8>>,
        target: Option<(u32, u32)>,
        hw: bool,
    ) -> Result<VideoPlayer, String> {
        let shared = Arc::new(Mutex::new(Shared {
            current: Arc::new(Raster {
                width: 0,
                height: 0,
                bgra: Vec::new(),
            }),
            status: Status::Ok,
            version: 0,
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let paused = Arc::new(AtomicBool::new(false));
        let s = shared.clone();
        let st = stop.clone();
        let p = paused.clone();

        let handle = thread::Builder::new()
            .name("strpaper-video".into())
            .spawn(move || run_loop(data, target, hw, s, st, p))
            .map_err(|e| format!("spawn decode thread failed: {e}"))?;

        Ok(VideoPlayer {
            shared,
            stop,
            handle: Some(handle),
            started: Instant::now(),
            hw,
            paused,
        })
    }

    /// Pause/resume decoding (used when the wallpaper is fully covered).
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    /// Return the most recently decoded frame (if any).
    pub fn frame_at(&mut self, _elapsed: Duration) -> Option<Arc<Raster>> {
        let g = self.shared.lock().ok()?;
        Some(g.current.clone())
    }

    /// Monotonic counter bumped every time a new frame is published, so the UI
    /// can skip repainting when the decoder hasn't produced anything new.
    pub fn version(&self) -> u64 {
        self.shared.lock().map(|g| g.version).unwrap_or(0)
    }

    /// True if the media source could not be decoded.
    pub fn is_failed(&self) -> bool {
        if let Ok(g) = self.shared.lock() {
            matches!(&g.status, Status::Failed(_))
        } else {
            false
        }
    }

    /// True when this player was opened with GPU decoding.
    pub fn is_hw(&self) -> bool {
        self.hw
    }

    /// The reason the media could not be decoded, if applicable.
    pub fn failure_reason(&self) -> Option<String> {
        if let Ok(g) = self.shared.lock() {
            match &g.status {
                Status::Failed(s) => Some(s.clone()),
                Status::Ok => None,
            }
        } else {
            None
        }
    }

    /// True once at least one real (non-empty) frame has been published.
    pub fn has_yielded(&self) -> bool {
        if let Ok(g) = self.shared.lock() {
            g.current.width > 0
        } else {
            false
        }
    }

    /// Seconds since the player was opened (used for a stall watchdog).
    pub fn active_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Detach the thread rather than join: if `ReadSample` is blocked the
        // join would hang the process. The OS reclaims the thread at exit.
        self.handle.take();
    }
}

fn mark_failed(shared: &Arc<Mutex<Shared>>, err: String) {
    if let Ok(mut g) = shared.lock() {
        g.status = Status::Failed(err);
    }
}

/// The background decode loop.
fn run_loop(
    data: Arc<Vec<u8>>,
    target: Option<(u32, u32)>,
    hw: bool,
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
) {
    // COM must be initialised on this thread before using Media Foundation.
    let _ = unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) };

    // The GPU device is created once and reused across end-of-stream reopens.
    let gpu = if hw {
        match crate::gpu::Gpu::new() {
            Ok(g) => Some(Arc::new(g)),
            Err(e) => {
                crate::logger::log(&format!("video: GPU unavailable ({e}); using software decode"));
                None
            }
        }
    } else {
        None
    };

    let mut reader: Option<Reader> = match Reader::new(&data, target, gpu.clone()) {
        Ok(r) => Some(r),
        Err(e) => {
            mark_failed(&shared, e);
            let _ = unsafe { CoUninitialize() };
            return;
        }
    };

    while !stop.load(Ordering::SeqCst) {
        // Fully covered by a maximized/fullscreen app: idle the decoder
        // instead of burning GPU/CPU on frames nobody can see.
        if paused.load(Ordering::SeqCst) {
            thread::sleep(Duration::from_millis(50));
            continue;
        }
        let Some(r) = reader.as_mut() else {
            break;
        };
        match r.next() {
            Next::Frame(frame) => {
                let dur = r.frame_dur;
                if let Ok(mut g) = shared.lock() {
                    g.status = Status::Ok;
                    g.version += 1;
                    g.current = Arc::new(frame);
                }
                thread::sleep(dur);
            }
            Next::EndOfStream => {
                reader = None; // recreate below to loop the source
            }
            Next::Again => {
                thread::sleep(THROTTLE);
            }
            Next::Error(reason) => {
                mark_failed(&shared, reason.clone());
                break;
            }
        }

        if reader.is_none() && !stop.load(Ordering::SeqCst) {
            match Reader::new(&data, target, gpu.clone()) {
                Ok(r) => reader = Some(r),
                Err(e) => {
                    mark_failed(&shared, e);
                    break;
                }
            }
            thread::sleep(Duration::from_millis(16));
        }
    }

    let _ = unsafe { CoUninitialize() };
}

enum Next {
    Frame(Raster),
    EndOfStream,
    Again,
    Error(String),
}

/// A single open source reader wrapped with our decoding helpers.
struct Reader {
    source: IMFSourceReader,
    stream_index: u32,
    width: u32,
    height: u32,
    frame_dur: Duration,
    fmt: PixelFormat,
    /// Output size to convert into (monitor resolution), if provided.
    target: Option<(u32, u32)>,
    /// GPU pipeline for hardware decode; `None` = software decode.
    gpu: Option<Arc<crate::gpu::Gpu>>,
}

/// The uncompressed pixel format the Source Reader delivers.
#[derive(Clone, Copy, PartialEq, Eq)]
enum PixelFormat {
    Rgb24,
    Yuy2,
    Nv12,
}

impl Reader {
    fn new(
        data: &[u8],
        target: Option<(u32, u32)>,
        gpu: Option<Arc<crate::gpu::Gpu>>,
    ) -> Result<Reader, String> {
        let (source, stream_index, width, height, fmt) =
            unsafe { init_reader(data, gpu.as_deref()) }?;
        Ok(Reader {
            source,
            stream_index,
            width,
            height,
            frame_dur: DEFAULT_FRAME_DUR,
            fmt,
            target,
            gpu,
        })
    }

    /// Read and decode the next video sample.
    fn next(&mut self) -> Next {
        unsafe {
            let mut _actual = 0u32;
            let mut stream_flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            let result = self.source.ReadSample(
                self.stream_index,
                0,
                Some(&mut _actual),
                Some(&mut stream_flags),
                None,
                Some(&mut sample),
            );
            if result.is_err() {
                return Next::Error(format!("ReadSample failed: {}", result.unwrap_err()));
            }

            let flags = stream_flags as i32;
            if (flags & MF_SOURCE_READERF_ERROR.0) != 0 {
                return Next::Error("media stream error".into());
            }
            if (flags & MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
                return Next::EndOfStream;
            }
            let Some(sample) = sample else {
                return Next::Again;
            };

            let dur = sample
                .GetSampleDuration()
                .ok()
                .filter(|d| *d > 0)
                .map(|d| Duration::from_nanos((d as u64).saturating_mul(TICKS_TO_NANOS)))
                .unwrap_or(DEFAULT_FRAME_DUR);
            if dur > Duration::ZERO {
                self.frame_dur = dur;
            }

            match decode_sample(
                &sample,
                self.width,
                self.height,
                self.fmt,
                self.target,
                self.gpu.as_deref(),
            ) {
                Ok(raster) => Next::Frame(raster),
                Err(e) => Next::Error(e),
            }
        }
    }
}

/// Open a source reader for `path`, select its first video stream and request a
/// **decoded** output format. With a GPU attached the reader is given a DXGI
/// device manager so the decoder runs on the video engine; otherwise the CPU
/// decoder is used and NV12/YUY2 is converted to BGRA in software.
/// Open a source reader over an **in-memory** copy of the media file. The
/// whole file lives in RAM, so looping playback never touches the disk.
unsafe fn init_reader(
    data: &[u8],
    gpu: Option<&crate::gpu::Gpu>,
) -> Result<(IMFSourceReader, u32, u32, u32, PixelFormat), String> { unsafe {
    // Wrap the bytes in an IStream and hand that to Media Foundation.
    let stream = SHCreateMemStream(Some(data)).ok_or("create memory stream failed")?;
    let bytestream = MFCreateMFByteStreamOnStream(&stream)
        .map_err(|e| format!("create byte stream failed: {e}"))?;

    // With a GPU present, attach the DXGI device manager so the H.264 decoder
    // runs on the video engine (hardware) instead of the CPU.
    let mut attrs: Option<windows::Win32::Media::MediaFoundation::IMFAttributes> = None;
    windows::Win32::Media::MediaFoundation::MFCreateAttributes(&mut attrs, 2)
        .map_err(|e| format!("create attributes failed: {e}"))?;
    let attrs = attrs.ok_or("create attributes failed".to_string())?;
    if let Some(gpu) = gpu {
        gpu.attach_to(&attrs)?;
    }

    let reader = MFCreateSourceReaderFromByteStream(&bytestream, Some(&attrs))
        .map_err(|e| format!("open media source failed: {e}"))?;

    // Find the first video stream.
    let mut video_index: Option<u32> = None;
    for i in 0..32u32 {
        match reader.GetCurrentMediaType(i) {
            Ok(mt) => {
                let major = mt.GetGUID(&MF_MT_MAJOR_TYPE).unwrap_or_else(|_| GUID::zeroed());
                if major == MFMediaType_Video {
                    video_index = Some(i);
                    break;
                }
            }
            Err(_) => break,
        }
    }
    let video_index = video_index.ok_or_else(|| "no video stream present".to_string())?;

    // Ensure the video stream is selected. (We intentionally do NOT deselect
    // the others; doing so can leave the media source in a bad state.)
    reader
        .SetStreamSelection(video_index, true)
        .map_err(|e| format!("select video stream failed: {e}"))?;

    // Frame size from the (compressed) native type, used to build a complete
    // output type so the decoder is actually inserted.
    let native = reader
        .GetNativeMediaType(video_index, 0)
        .map_err(|e| format!("read native media type failed: {e}"))?;
    let packed_size = native
        .GetUINT64(&MF_MT_FRAME_SIZE)
        .map_err(|e| format!("read frame size failed: {e}"))?;
    // MF_MT_FRAME_SIZE packs HEIGHT in the low DWORD and WIDTH in the high
    // DWORD — getting this backwards yields a sheared/"combined frames" image.
    let width = (packed_size >> 32) as u32;
    let height = (packed_size & 0xFFFF_FFFF) as u32;
    if width == 0 || height == 0 {
        return Err("failed to determine video dimensions".into());
    }

    // Try decoded output formats in preference order (all producible without a
    // colour-conversion MFT). The first accepted one is used.
    let candidates: [(GUID, PixelFormat); 3] = [
        (MFVideoFormat_NV12, PixelFormat::Nv12),
        (MFVideoFormat_YUY2, PixelFormat::Yuy2),
        (MFVideoFormat_RGB24, PixelFormat::Rgb24),
    ];
    for (sub, fmt) in candidates {
        let mt = build_output_type(packed_size, sub)?;
        if reader.SetCurrentMediaType(video_index, None, &mt).is_ok() {
            return Ok((reader, video_index, width, height, fmt));
        }
    }
    Err("no decodable output format available (codec not supported by Media Foundation)".into())
}}

unsafe fn build_output_type(packed_size: u64, subtype: GUID) -> Result<IMFMediaType, String> { unsafe {
    let output = MFCreateMediaType().map_err(|e| format!("create media type failed: {e}"))?;
    output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
    output.SetGUID(&MF_MT_SUBTYPE, &subtype).ok();
    output.SetUINT64(&MF_MT_FRAME_SIZE, packed_size).ok();
    Ok(output)
}}

/// Convert a decoded media sample into a top-down BGRA raster.
///
/// With a GPU attached the decoded NV12 surface never touches CPU memory until
/// after the video processor has converted it to BGRA on the GPU — only the
/// finished frame is copied back. Without one, the sample's packed surface is
/// converted in software.
///
/// When `target` differs from the source size, frames are converted straight
/// into that size (nearest sampling) so we never convert or blit more pixels
/// than the screen can show.
unsafe fn decode_sample(
    sample: &IMFSample,
    width: u32,
    height: u32,
    fmt: PixelFormat,
    target: Option<(u32, u32)>,
    gpu: Option<&crate::gpu::Gpu>,
) -> Result<Raster, String> { unsafe {
    let (w, h) = (width as usize, height as usize);

    // Output size: the requested monitor size, clamped to the source (never
    // upscale: it wastes CPU and adds nothing for a wallpaper).
    let (out_w, out_h) = match target {
        Some((tw, th)) if tw > 0 && th > 0 => ((tw as usize).min(w), (th as usize).min(h)),
        _ => (w, h),
    };

    // GPU path: hardware decode + video-processor colour conversion.
    if let Some(gpu) = gpu {
        if fmt == PixelFormat::Nv12 {
            let bgra = gpu
                .nv12_sample_to_bgra(sample, out_w, out_h)
                .map_err(|e| format!("gpu: {e}"))?;
            return Ok(Raster {
                width: out_w,
                height: out_h,
                bgra,
            });
        }
        // Non-NV12 with GPU present: fall through to software conversion.
    }

    if sample.GetBufferCount().unwrap_or(0) == 0 {
        return Err("sample has no buffers".into());
    }
    let buffer = sample.GetBufferByIndex(0).map_err(|_| "no sample buffer".to_string())?;
    let two_d = buffer
        .cast::<IMF2DBuffer>()
        .map_err(|_| "not a 2d buffer".to_string())?;
    let packed_len = two_d
        .GetContiguousLength()
        .map_err(|e| format!("contiguous length failed: {e}"))? as usize;
    let mut packed = vec![0u8; packed_len];
    two_d
        .ContiguousCopyTo(&mut packed)
        .map_err(|_| "surface copy failed".to_string())?;

    let dst_row = out_w * 4;
    let mut dst = vec![0u8; dst_row * out_h];

    // Nearest-neighbour source lookup tables, computed once per frame.
    let xs: Vec<usize> = (0..out_w).map(|x| x * w / out_w).collect();
    let ys: Vec<usize> = (0..out_h).map(|y| y * h / out_h).collect();

    match fmt {
        PixelFormat::Nv12 => nv12_to_bgra(&packed, w, h, &xs, &ys, dst.as_mut_slice()),
        PixelFormat::Yuy2 => yuy2_to_bgra(&packed, w, h, &xs, &ys, dst.as_mut_slice()),
        PixelFormat::Rgb24 => rgb24_to_bgra(&packed, w, h, &xs, &ys, dst.as_mut_slice()),
    }

    Ok(Raster {
        width: out_w,
        height: out_h,
        bgra: dst,
    })
}}

/// NV12 (4:2:0 — Y plane then interleaved UV plane), packed to width.
fn nv12_to_bgra(
    bytes: &[u8],
    w: usize,
    h: usize,
    xs: &[usize],
    ys: &[usize],
    dst: &mut [u8],
) {
    let dst_row = xs.len() * 4;
    let uv_base = w * h;
    for (row, &sy) in ys.iter().enumerate() {
        let dbase = row * dst_row;
        let y_row_off = sy * w;
        let uv_off = uv_base + (sy / 2) * w;
        for (col, &sx) in xs.iter().enumerate() {
            let y = *bytes.get(y_row_off + sx).unwrap_or(&0);
            let u = *bytes.get(uv_off + (sx & !1)).unwrap_or(&0);
            let v = *bytes.get(uv_off + (sx & !1) + 1).unwrap_or(&0);
            let (r, g, b) = yuv_to_rgb(y, u, v);
            let d = dbase + col * 4;
            dst[d] = b;
            dst[d + 1] = g;
            dst[d + 2] = r;
            dst[d + 3] = 255;
        }
    }
}

/// YUY2 (4:2:2), one macro-pixel is 4 bytes for 2 pixels.
fn yuy2_to_bgra(
    bytes: &[u8],
    w: usize,
    _h: usize,
    xs: &[usize],
    ys: &[usize],
    dst: &mut [u8],
) {
    let dst_row = xs.len() * 4;
    for (row, &sy) in ys.iter().enumerate() {
        let dbase = row * dst_row;
        let base = sy * w * 2;
        for (col, &sx) in xs.iter().enumerate() {
            let o = base + (sx & !1) * 2;
            let y = *bytes.get(o).unwrap_or(&0);
            let u = *bytes.get(o + 1).unwrap_or(&0);
            let v = *bytes.get(o + 3).unwrap_or(&0);
            let (r, g, b) = yuv_to_rgb(y, u, v);
            let d = dbase + col * 4;
            dst[d] = b;
            dst[d + 1] = g;
            dst[d + 2] = r;
            dst[d + 3] = 255;
        }
    }
}

/// RGB24 (BGR, 3 bytes/pixel).
fn rgb24_to_bgra(
    bytes: &[u8],
    w: usize,
    _h: usize,
    xs: &[usize],
    ys: &[usize],
    dst: &mut [u8],
) {
    let dst_row = xs.len() * 4;
    for (row, &sy) in ys.iter().enumerate() {
        let dbase = row * dst_row;
        let base = sy * w * 3;
        for (col, &sx) in xs.iter().enumerate() {
            let s = base + sx * 3;
            let d = dbase + col * 4;
            dst[d] = *bytes.get(s).unwrap_or(&0); // B
            dst[d + 1] = *bytes.get(s + 1).unwrap_or(&0); // G
            dst[d + 2] = *bytes.get(s + 2).unwrap_or(&0); // R
            dst[d + 3] = 255;
        }
    }
}

/// yuv_to_rgb using BT.601 limited-range coefficients.
fn yuv_to_rgb(y: u8, u: u8, v: u8) -> (u8, u8, u8) {
    let c = y as i32 - 16;
    let d = u as i32 - 128;
    let e = v as i32 - 128;
    let r = (298 * c + 409 * e + 128) >> 8;
    let g = (298 * c - 100 * d - 208 * e + 128) >> 8;
    let b = (298 * c + 516 * d + 128) >> 8;
    (
        r.clamp(0, 255) as u8,
        g.clamp(0, 255) as u8,
        b.clamp(0, 255) as u8,
    )
}

/// Initializes COM on the current thread and uninitializes it on drop.
struct ComGuard;

impl ComGuard {
    #[allow(dead_code)] // kept for symmetry; transcode() uses `let _com = ComGuard;`
    fn new() -> ComGuard {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        }
        ComGuard
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        unsafe {
            let _ = CoUninitialize();
        }
    }
}

/// H.264 output type for the transcode sink writer.
unsafe fn transcode_output_type(
    w: u32,
    h: u32,
) -> Result<IMFMediaType, String> { unsafe {
    let mt = MFCreateMediaType().map_err(|e| format!("create media type failed: {e}"))?;
    mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
    mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ok();
    mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_frame_size(w, h)).ok();
    // ~0.07 bits per pixel per frame at 30 fps, clamped to sane bounds.
    let bitrate = ((w as u64 * h as u64 * 30 * 7) / 100).clamp(1_000_000, 40_000_000) as u32;
    mt.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).ok();
    mt.SetUINT64(&MF_MT_FRAME_RATE, pack_frame_size(30, 1)).ok();
    Ok(mt)
}}

/// RGB32 input type matching our decoded rasters.
unsafe fn transcode_input_type(
    w: u32,
    h: u32,
) -> Result<IMFMediaType, String> { unsafe {
    let mt = MFCreateMediaType().map_err(|e| format!("create media type failed: {e}"))?;
    mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
    mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32).ok();
    mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_frame_size(w, h)).ok();
    Ok(mt)
}}

fn pack_frame_size(w: u32, h: u32) -> u64 {
    (h as u64) | ((w as u64) << 32)
}

/// Wrap a packed BGRA frame into an IMFSample and write it to the sink.
unsafe fn write_bgra_sample(
    sink: &windows::Win32::Media::MediaFoundation::IMFSinkWriter,
    idx: u32,
    bgra: &[u8],
    time100ns: i64,
    dur100ns: i64,
) -> Result<(), String> { unsafe {
    let buffer =
        MFCreateMemoryBuffer(bgra.len() as u32).map_err(|e| format!("buffer failed: {e}"))?;
    let mut ptr: *mut u8 = std::ptr::null_mut();
    let mut max_len = 0u32;
    let mut cur_len = 0u32;
    buffer
        .Lock(&mut ptr, Some(&mut max_len), Some(&mut cur_len))
        .map_err(|e| format!("buffer lock failed: {e}"))?;
    if !ptr.is_null() {
        std::slice::from_raw_parts_mut(ptr, bgra.len())
            .copy_from_slice(bgra);
        let _ = buffer.SetCurrentLength(bgra.len() as u32);
    }
    let _ = buffer.Unlock();

    let sample = MFCreateSample().map_err(|e| format!("sample failed: {e}"))?;
    sample.AddBuffer(&buffer).map_err(|e| format!("add buffer failed: {e}"))?;
    sample.SetSampleTime(time100ns).map_err(|e| format!("sample time failed: {e}"))?;
    sample.SetSampleDuration(dur100ns).map_err(|e| format!("sample duration failed: {e}"))?;
    sink.WriteSample(idx, &sample).map_err(|e| format!("write sample failed: {e}"))
}}
