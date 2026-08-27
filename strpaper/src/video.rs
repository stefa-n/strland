//! Media Foundation video playback for MP4/WebM — background decoding.

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

const DEFAULT_FRAME_DUR: Duration = Duration::from_millis(33);
const TICKS_TO_NANOS: u64 = 100;
const THROTTLE: Duration = Duration::from_millis(4);

/// Init Media Foundation.
pub fn startup() -> Result<(), String> {
    unsafe { MFStartup(MF_VERSION, 0) }.map_err(|e| format!("MFStartup failed: {e}"))
}

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

/// Live video player (background thread).
pub struct VideoPlayer {
    shared: Arc<Mutex<Shared>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    started: Instant,
    hw: bool,
    paused: Arc<AtomicBool>,
}

impl VideoPlayer {
    /// Probe dimensions without playback.
    pub fn probe_size(data: &[u8]) -> Option<(u32, u32)> {
        let _com = ComGuard;
        let (_, _, w, h, _) = unsafe { init_reader(data, None) }.ok()?;
        Some((w, h))
    }

    /// Transcode in-memory video to H.264 (blocking).
    pub fn transcode(
        data: Arc<Vec<u8>>,
        out_w: u32,
        out_h: u32,
    ) -> Result<Vec<u8>, String> {
        let _com = ComGuard;
        let (out_w, out_h) = (out_w | 1, out_h | 1); // H.264 needs even sizes
        let mut reader = Reader::new(&data, Some((out_w, out_h)), None)?;

        unsafe {
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

    /// Open video (non-blocking, decodes to `target` size).
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

    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::SeqCst);
    }

    pub fn frame_at(&self, _elapsed: Duration) -> Option<Arc<Raster>> {
        let g = self.shared.lock().ok()?;
        Some(g.current.clone())
    }

    pub fn version(&self) -> u64 {
        self.shared.lock().map(|g| g.version).unwrap_or(0)
    }

    pub fn is_failed(&self) -> bool {
        if let Ok(g) = self.shared.lock() {
            matches!(&g.status, Status::Failed(_))
        } else {
            false
        }
    }

    pub fn is_hw(&self) -> bool {
        self.hw
    }

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

    pub fn has_yielded(&self) -> bool {
        if let Ok(g) = self.shared.lock() {
            g.current.width > 0
        } else {
            false
        }
    }

    pub fn active_secs(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    /// Stop thread, waiting up to `timeout`.
    pub fn close(&mut self, timeout: Duration) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + timeout;
            while !h.is_finished() {
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
    }
}

impl Drop for VideoPlayer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let deadline = Instant::now() + Duration::from_secs(2);
            while !h.is_finished() {
                if Instant::now() >= deadline {
                    break;
                }
                std::thread::sleep(Duration::from_millis(5));
            }
            if h.is_finished() {
                let _ = h.join();
            }
        }
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

    drop(reader);
    drop(gpu);
    drop(data);
    let _ = unsafe { CoUninitialize() };
}

enum Next {
    Frame(Raster),
    EndOfStream,
    Again,
    Error(String),
}

struct Reader {
    source: IMFSourceReader,
    stream_index: u32,
    width: u32,
    height: u32,
    frame_dur: Duration,
    fmt: PixelFormat,
    target: Option<(u32, u32)>,
    gpu: Option<Arc<crate::gpu::Gpu>>,
}

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

/// Open source reader over in-memory bytes.
unsafe fn init_reader(
    data: &[u8],
    gpu: Option<&crate::gpu::Gpu>,
) -> Result<(IMFSourceReader, u32, u32, u32, PixelFormat), String> { unsafe {
    let stream = SHCreateMemStream(Some(data)).ok_or("create memory stream failed")?;
    let bytestream = MFCreateMFByteStreamOnStream(&stream)
        .map_err(|e| format!("create byte stream failed: {e}"))?;

    let mut attrs: Option<windows::Win32::Media::MediaFoundation::IMFAttributes> = None;
    windows::Win32::Media::MediaFoundation::MFCreateAttributes(&mut attrs, 2)
        .map_err(|e| format!("create attributes failed: {e}"))?;
    let attrs = attrs.ok_or("create attributes failed".to_string())?;
    if let Some(gpu) = gpu {
        gpu.attach_to(&attrs)?;
    }

    let reader = MFCreateSourceReaderFromByteStream(&bytestream, Some(&attrs))
        .map_err(|e| format!("open media source failed: {e}"))?;

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

    reader
        .SetStreamSelection(video_index, true)
        .map_err(|e| format!("select video stream failed: {e}"))?;

    let native = reader
        .GetNativeMediaType(video_index, 0)
        .map_err(|e| format!("read native media type failed: {e}"))?;
    let packed_size = native
        .GetUINT64(&MF_MT_FRAME_SIZE)
        .map_err(|e| format!("read frame size failed: {e}"))?;
    let width = (packed_size >> 32) as u32;
    let height = (packed_size & 0xFFFF_FFFF) as u32;
    if width == 0 || height == 0 {
        return Err("failed to determine video dimensions".into());
    }

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

unsafe fn decode_sample(
    sample: &IMFSample,
    width: u32,
    height: u32,
    fmt: PixelFormat,
    target: Option<(u32, u32)>,
    gpu: Option<&crate::gpu::Gpu>,
) -> Result<Raster, String> { unsafe {
    let (w, h) = (width as usize, height as usize);

    let (out_w, out_h) = match target {
        Some((tw, th)) if tw > 0 && th > 0 => ((tw as usize).min(w), (th as usize).min(h)),
        _ => (w, h),
    };

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

unsafe fn transcode_output_type(
    w: u32,
    h: u32,
) -> Result<IMFMediaType, String> { unsafe {
    let mt = MFCreateMediaType().map_err(|e| format!("create media type failed: {e}"))?;
    mt.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video).ok();
    mt.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_H264).ok();
    mt.SetUINT64(&MF_MT_FRAME_SIZE, pack_frame_size(w, h)).ok();
    let bitrate = ((w as u64 * h as u64 * 30 * 7) / 100).clamp(1_000_000, 40_000_000) as u32;
    mt.SetUINT32(&MF_MT_AVG_BITRATE, bitrate).ok();
    mt.SetUINT64(&MF_MT_FRAME_RATE, pack_frame_size(30, 1)).ok();
    Ok(mt)
}}

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
