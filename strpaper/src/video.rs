//! Media Foundation video playback for MP4 / WebM wallpapers.
//!
//! Uses the Source Reader (`IMFSourceReader`) so the decoder and colour
//! conversion are provided by the OS (hardware accelerated where available).
//! Frames are converted to a top-down BGRA [`crate::render::Raster`] and
//! repainted by the caller.
//!
//! If a codec is unavailable (or the file is malformed) the player degrades
//! gracefully: no frames are produced and the caller clears the desktop.

use std::path::{Path, PathBuf};
use std::ptr;
use std::time::{Duration, Instant};

use windows::core::{GUID, Interface};
use windows::Win32::Media::MediaFoundation::{
    IMF2DBuffer, IMFMediaBuffer, IMFSample, IMFSourceReader, MFCreateMediaType,
    MFCreateSourceReaderFromURL, MFShutdown, MFStartup, MFMediaType_Video, MF_MT_FRAME_SIZE,
    MF_MT_MAJOR_TYPE, MF_MT_SUBTYPE, MF_SOURCE_READERF_ENDOFSTREAM, MF_SOURCE_READERF_ERROR,
    MFVideoFormat_RGB32, MF_VERSION,
};

use crate::render::Raster;

/// Default frame duration used when the media provides no timing.
const DEFAULT_FRAME_DUR: Duration = Duration::from_millis(33);
/// Media Foundation presents sample durations in 100ns ticks.
const TICKS_TO_NANOS: u64 = 100;

/// Initialize Media Foundation once for the process.
pub fn startup() -> Result<(), String> {
    unsafe { MFStartup(MF_VERSION, 0) }.map_err(|e| format!("MFStartup failed: {e}"))
}

/// Shut down Media Foundation.
pub fn shutdown() {
    let _ = unsafe { MFShutdown() };
}

/// A live video wallpaper player.
pub struct VideoPlayer {
    reader: IMFSourceReader,
    stream_index: u32,
    width: u32,
    height: u32,
    path: PathBuf,
    /// The currently displayed frame.
    current: Raster,
    /// How long to keep displaying `current` before moving to the next frame.
    frame_dur: Duration,
    /// Wall-clock moment `current` started being displayed.
    frame_started: Instant,
    /// End of stream reached; the next tick loops the source.
    eof: bool,
}

impl VideoPlayer {
    /// Open a video file for playback.
    pub fn open(path: &Path) -> Result<VideoPlayer, String> {
        let (reader, stream_index, width, height) = unsafe { init_reader(path)? };
        let (w, h) = (width as usize, height as usize);
        let mut player = VideoPlayer {
            reader,
            stream_index,
            width,
            height,
            path: path.to_path_buf(),
            current: Raster {
                width: w,
                height: h,
                bgra: vec![0u8; w * h * 4],
            },
            frame_dur: DEFAULT_FRAME_DUR,
            frame_started: Instant::now(),
            eof: false,
        };
        // Prime the first frame so there is always something to show.
        let _ = player.read_next();
        Ok(player)
    }

    /// Return the raster that should be displayed right now.
    ///
    /// The player paces playback on a wall-clock basis (it ignores `elapsed`);
    /// frames advance at the media's native cadence and loop at the end.
    pub fn frame_at(&mut self, _elapsed: Duration) -> Option<&Raster> {
        let _ = self.tick();
        Some(&self.current)
    }

    /// Advance the display to the colour frame the wall-clock says is due.
    fn tick(&mut self) -> Result<(), ()> {
        loop {
            if self.eof {
                let reopened = unsafe { init_reader(&self.path) };
                match reopened {
                    Ok((reader, stream_index, width, height)) => {
                        self.reader = reader;
                        self.stream_index = stream_index;
                        self.width = width;
                        self.height = height;
                        self.eof = false;
                    }
                    Err(_) => return Err(()),
                }
                continue;
            }

            // Not yet time to move to the next frame.
            if self.frame_started.elapsed() < self.frame_dur {
                break;
            }

            match self.read_next() {
                Ok(Some(())) => {
                    self.frame_started = Instant::now();
                }
                // Nothing new available; keep showing the current frame.
                Ok(None) => break,
                Err(_) => return Err(()),
            }
        }
        Ok(())
    }

    /// Read and decode a single video sample into [`Self::current`].
    ///
    /// Returns `Ok(Some(()))` if a new frame was decoded, `Ok(None)` when there
    /// is nothing available, and `Err` on a hard decode failure.
    fn read_next(&mut self) -> Result<Option<()>, ()> {
        unsafe {
            let mut actual = 0u32;
            let mut stream_flags = 0u32;
            let mut sample: Option<IMFSample> = None;
            self.reader
                .ReadSample(
                    self.stream_index,
                    0,
                    Some(&mut actual),
                    Some(&mut stream_flags),
                    None,
                    Some(&mut sample),
                )
                .map_err(|_| ())?;

            let flags_i = stream_flags as i32;
            if (flags_i & MF_SOURCE_READERF_ERROR.0) != 0 {
                return Err(());
            }
            if (flags_i & MF_SOURCE_READERF_ENDOFSTREAM.0) != 0 {
                self.eof = true;
                return Ok(None);
            }
            let Some(sample) = sample else {
                return Ok(None);
            };

            let dur = sample
                .GetSampleDuration()
                .ok()
                .filter(|d| *d > 0)
                .map(|d| Duration::from_nanos((d as u64).saturating_mul(TICKS_TO_NANOS)))
                .unwrap_or(DEFAULT_FRAME_DUR);

            let raster = decode_sample(&sample, self.width, self.height)?;
            self.current = raster;
            self.frame_dur = dur;
            Ok(Some(()))
        }
    }
}

/// Open a source reader for `path`, select its first video stream and request
/// 32-bit RGB output. Returns `(reader, stream_index, width, height)`.
unsafe fn init_reader(path: &Path) -> Result<(IMFSourceReader, u32, u32, u32), String> { unsafe {
    let url: Vec<u16> = path
        .to_string_lossy()
        .encode_utf16()
        .chain(std::iter::once(0))
        .collect();
    let reader = MFCreateSourceReaderFromURL(windows::core::PCWSTR(url.as_ptr()), None)
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

    // Request uncompressed RGB output; the Source Reader inserts a converter.
    let output = MFCreateMediaType().map_err(|e| format!("create media type failed: {e}"))?;
    let _ = output.SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video);
    let _ = output.SetGUID(&MF_MT_SUBTYPE, &MFVideoFormat_RGB32);
    reader
        .SetCurrentMediaType(video_index, None, &output)
        .map_err(|e| format!("set RGB32 output failed: {e}"))?;

    let out = reader
        .GetCurrentMediaType(video_index)
        .map_err(|e| format!("read output media type failed: {e}"))?;
    let packed = out.GetUINT64(&MF_MT_FRAME_SIZE).ok();
    let (width, height) = match packed {
        Some(p) => ((p & 0xFFFF_FFFF) as u32, (p >> 32) as u32),
        None => (0, 0),
    };
    if width == 0 || height == 0 {
        return Err("failed to determine video dimensions".into());
    }

    Ok((reader, video_index, width, height))
}}

/// Convert a decoded media sample into a top-down BGRA raster.
unsafe fn decode_sample(
    sample: &IMFSample,
    width: u32,
    height: u32,
) -> Result<Raster, ()> { unsafe {
    let (w, h) = (width as usize, height as usize);
    let row_bytes = w * 4;
    if sample.GetBufferCount().unwrap_or(0) == 0 {
        return Err(());
    }
    let buffer = sample.GetBufferByIndex(0).map_err(|_| ())?;
    let mut dst = vec![0u8; row_bytes * h];

    // Prefer the 2D buffer so the true pixel pitch is respected.
    if let Ok(two_d) = buffer.cast::<IMF2DBuffer>() {
        let mut base: *mut u8 = ptr::null_mut();
        let mut pitch: i32 = 0;
        two_d.Lock2D(&mut base, &mut pitch).map_err(|_| ())?;
        if !base.is_null() && pitch > 0 {
            let pitch = pitch as usize;
            let src = std::slice::from_raw_parts(base, pitch * h);
            for row in 0..h {
                let src_off = (h - 1 - row) * pitch; // bottom-up -> top-down
                let src_end = (src_off + row_bytes).min(src.len());
                dst[row * row_bytes..row * row_bytes + (src_end - src_off)]
                    .copy_from_slice(&src[src_off..src_end]);
            }
        }
        let _ = two_d.Unlock2D();
        return Ok(Raster {
            width: w,
            height: h,
            bgra: dst,
        });
    }

    // Fallback: contiguous buffer, assume packed bottom-up rows.
    let contiguous = buffer.cast::<IMFMediaBuffer>().map_err(|_| ())?;
    let mut data: *mut u8 = ptr::null_mut();
    let mut max_len = 0u32;
    let mut len = 0u32;
    contiguous
        .Lock(&mut data, Some(&mut max_len), Some(&mut len))
        .map_err(|_| ())?;
    if !data.is_null() {
        let len = (len as usize).min(row_bytes * h);
        let src = std::slice::from_raw_parts(data, len);
        for row in 0..h {
            let src_off = (h - 1 - row) * row_bytes;
            if src_off + row_bytes <= len {
                dst[row * row_bytes..(row + 1) * row_bytes]
                    .copy_from_slice(&src[src_off..src_off + row_bytes]);
            }
        }
    }
    let _ = contiguous.Unlock();
    Ok(Raster {
        width: w,
        height: h,
        bgra: dst,
    })
}}
