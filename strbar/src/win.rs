#[cfg(target_os = "windows")]
use std::os::windows::ffi::OsStrExt;
#[cfg(target_os = "windows")]
use std::path::Path;
#[cfg(target_os = "windows")]
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
#[cfg(target_os = "windows")]
use std::sync::OnceLock;

// ---------------------------------------------------------------------------
// Raw FFI declarations
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
unsafe extern "system" {
    fn GetSystemMetrics(n_index: i32) -> i32;
    fn GetForegroundWindow() -> isize;
    fn IsZoomed(hWnd: isize) -> i32;
    fn GetWindowRect(hWnd: isize, lpRect: *mut RECT) -> i32;
    fn GetCursorPos(lp_point: *mut POINT) -> i32;
    fn GetTickCount64() -> u64;
    fn GetDpiForWindow(hWnd: isize) -> u32;
    fn SetWindowPos(
        hWnd: isize,
        hWnd_insert_after: isize,
        x: i32,
        y: i32,
        cx: i32,
        cy: i32,
        u_flags: u32,
    ) -> i32;
    fn SetWindowLongPtrW(hWnd: isize, nIndex: i32, dwNewLong: isize) -> isize;
    fn GetWindowLongPtrW(hWnd: isize, nIndex: i32) -> isize;
    fn SetForegroundWindow(hWnd: isize) -> i32;
    fn AttachThreadInput(id_attach: u32, id_attach_to: u32, f_attach: i32) -> i32;
    fn GetCurrentThreadId() -> u32;
    fn waveOutGetVolume(hwo: usize, pdw_volume: *mut u32) -> u32;
    fn ShellExecuteW(
        hwnd: isize,
        lpOperation: *const u16,
        lpFile: *const u16,
        lpParameters: *const u16,
        lpDirectory: *const u16,
        nShowCmd: i32,
    ) -> isize;
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct POINT {
    x: i32,
    y: i32,
}

#[cfg(target_os = "windows")]
#[repr(C)]
#[derive(Clone, Copy, Default)]
struct RECT {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
}

#[cfg(target_os = "windows")]
const SM_CXSCREEN: i32 = 0;
#[cfg(target_os = "windows")]
const SM_CYSCREEN: i32 = 1;
#[cfg(target_os = "windows")]
const HWND_TOPMOST: isize = -1isize;
#[cfg(target_os = "windows")]
const SWP_NOSIZE: u32 = 0x0001;
#[cfg(target_os = "windows")]
const SWP_NOMOVE: u32 = 0x0002;
#[cfg(target_os = "windows")]
const SWP_NOACTIVATE: u32 = 0x0010;
#[cfg(target_os = "windows")]
const SWP_SHOWWINDOW: u32 = 0x0040;

// ---------------------------------------------------------------------------
// Cached screen width (never changes during a session)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub fn get_screen_width() -> i32 {
    fn cached() -> i32 {
        static WIDTH: OnceLock<i32> = OnceLock::new();
        *WIDTH.get_or_init(|| unsafe { GetSystemMetrics(SM_CXSCREEN) })
    }
    cached()
}

// ---------------------------------------------------------------------------
// Window positioning – dedup calls to avoid redundant DWM recomposition
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod last_pos {
    use std::cell::Cell;
    thread_local! {
        static X: Cell<i32> = const { Cell::new(i32::MIN) };
        static Y: Cell<i32> = const { Cell::new(i32::MIN) };
        static W: Cell<i32> = const { Cell::new(i32::MIN) };
        static H: Cell<i32> = const { Cell::new(i32::MIN) };
    }

    pub fn changed(x: i32, y: i32, w: i32, h: i32) -> bool {
        X.with(|v| v.get()) != x
            || Y.with(|v| v.get()) != y
            || W.with(|v| v.get()) != w
            || H.with(|v| v.get()) != h
    }

    pub fn save(x: i32, y: i32, w: i32, h: i32) {
        X.with(|v| v.set(x));
        Y.with(|v| v.set(y));
        W.with(|v| v.set(w));
        H.with(|v| v.set(h));
    }
}

#[cfg(target_os = "windows")]
pub fn set_position(hwnd: isize, x: i32, y: i32) {
    if !last_pos::changed(x, y, 0, 0) {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            0,
            0,
            SWP_NOSIZE | SWP_SHOWWINDOW | SWP_NOACTIVATE,
        );
    }
    last_pos::save(x, y, 0, 0);
}

#[cfg(target_os = "windows")]
pub fn resize_window(hwnd: isize, width: i32, height: i32) {
    if !last_pos::changed(0, 0, width, height) {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            0,
            0,
            width,
            height,
            SWP_NOMOVE | SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    last_pos::save(0, 0, width, height);
}

#[cfg(target_os = "windows")]
pub fn set_position_and_size(hwnd: isize, x: i32, y: i32, width: i32, height: i32) {
    if !last_pos::changed(x, y, width, height) {
        return;
    }
    unsafe {
        SetWindowPos(
            hwnd,
            HWND_TOPMOST,
            x,
            y,
            width,
            height,
            SWP_NOACTIVATE | SWP_SHOWWINDOW,
        );
    }
    last_pos::save(x, y, width, height);
}

#[cfg(target_os = "windows")]
pub fn reset_pos_cache() {
    last_pos::save(i32::MIN, i32::MIN, i32::MIN, i32::MIN);
}

#[cfg(target_os = "windows")]
pub fn get_screen_height() -> i32 {
    unsafe { GetSystemMetrics(SM_CYSCREEN) }
}

/// Logical points → physical pixels factor for the given window (1.25 at 125% scaling).
#[cfg(target_os = "windows")]
pub fn window_scale(hwnd: isize) -> f32 {
    if hwnd == 0 {
        return 1.0;
    }
    let dpi = unsafe { GetDpiForWindow(hwnd) };
    if dpi == 0 {
        1.0
    } else {
        dpi as f32 / 96.0
    }
}

// ---------------------------------------------------------------------------
// Window state queries
// ---------------------------------------------------------------------------

/// The HWND of the currently foreground window, or 0.
#[cfg(target_os = "windows")]
pub fn foreground_hwnd() -> isize {
    unsafe { GetForegroundWindow() }
}

/// True when the foreground window is maximized OR spans the whole monitor
/// (borderless fullscreen video/games) — the island should hide in both cases.
#[cfg(target_os = "windows")]
pub fn foreground_covers_screen() -> bool {
    unsafe {
        let fg = GetForegroundWindow();
        if fg == 0 {
            return false;
        }
        if IsZoomed(fg) != 0 {
            return true;
        }
        let mut rect = RECT::default();
        if GetWindowRect(fg, &mut rect) == 0 {
            return false;
        }
        let screen_w = GetSystemMetrics(SM_CXSCREEN);
        let screen_h = GetSystemMetrics(SM_CYSCREEN);
        // Borderless fullscreen: window edges reach (or exceed) the monitor.
        rect.left <= 0 && rect.top <= 0 && rect.right >= screen_w && rect.bottom >= screen_h
    }
}

/// Call once per frame and pass results around – avoids redundant GetCursorPos.
pub struct FrameCursor {
    pub screen_x: i32,
    pub screen_y: i32,
}

#[cfg(target_os = "windows")]
pub fn frame_cursor() -> FrameCursor {
    unsafe {
        let mut pt = POINT::default();
        GetCursorPos(&mut pt);
        FrameCursor {
            screen_x: pt.x,
            screen_y: pt.y,
        }
    }
}

#[cfg(target_os = "windows")]
pub fn cursor_at_top(cursor: &FrameCursor, threshold: i32) -> bool {
    cursor.screen_y < threshold
}

#[cfg(target_os = "windows")]
pub fn configure_window_styles(hwnd: isize, clickthrough: bool, accepts_focus: bool) {
    const GWL_EXSTYLE: i32 = -20;
    const GWL_STYLE: i32 = -16;
    const WS_EX_TOOLWINDOW: isize = 0x00000080;
    const WS_EX_APPWINDOW: isize = 0x00040000;
    const WS_EX_NOACTIVATE: isize = 0x08000000;
    const WS_EX_TRANSPARENT: isize = 0x00000020;
    const WS_MINIMIZEBOX: isize = 0x00020000;
    const WS_MAXIMIZEBOX: isize = 0x00010000;
    const WS_THICKFRAME: isize = 0x00040000;

    unsafe {
        let ex_style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let mut new_ex_style = (ex_style | WS_EX_TOOLWINDOW) & !WS_EX_APPWINDOW;
        if !accepts_focus {
            new_ex_style |= WS_EX_NOACTIVATE;
        } else {
            new_ex_style &= !WS_EX_NOACTIVATE;
        }
        if clickthrough {
            new_ex_style |= WS_EX_TRANSPARENT;
        } else {
            new_ex_style &= !WS_EX_TRANSPARENT;
        }
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, new_ex_style);

        let style = GetWindowLongPtrW(hwnd, GWL_STYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_STYLE,
            style & !WS_MINIMIZEBOX & !WS_MAXIMIZEBOX & !WS_THICKFRAME,
        );
    }
}

#[cfg(target_os = "windows")]
pub fn focus_window(hwnd: isize) {
    unsafe {
        SetForegroundWindow(hwnd);
    }
}

// ---------------------------------------------------------------------------
// Volume – cached COM endpoint (thread_local)
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
mod cached_volume {
    use windows::Win32::Media::Audio::Endpoints::IAudioEndpointVolume;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    thread_local! {
        static CACHED: std::cell::RefCell<Option<IAudioEndpointVolume>> = const { std::cell::RefCell::new(None) };
    }

    fn ensure_endpoint() -> Option<IAudioEndpointVolume> {
        CACHED.with(|cell| {
            if let Some(ref ep) = *cell.borrow() {
                return Some(ep.clone());
            }
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                let enumerator: IMMDeviceEnumerator =
                    CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL).ok()?;
                let device =
                    enumerator.GetDefaultAudioEndpoint(eRender, eConsole).ok()?;
                let endpoint: IAudioEndpointVolume =
                    device.Activate(CLSCTX_ALL, None).ok()?;
                *cell.borrow_mut() = Some(endpoint.clone());
                Some(endpoint)
            }
        })
    }

    pub fn read_volume() -> Option<f32> {
        with_endpoint(|ep| unsafe { ep.GetMasterVolumeLevelScalar() }.ok().map(|v| v.clamp(0.0, 1.0)))
            .flatten()
    }

    fn with_endpoint<T>(f: impl FnOnce(&IAudioEndpointVolume) -> T) -> Option<T> {
        let ep = ensure_endpoint()?;
        Some(f(&ep))
    }

    /// Steps master volume up/down using the system's step size.
    pub fn step_volume(up: bool) -> bool {
        with_endpoint(|ep| {
            let guid = windows::core::GUID::zeroed();
            unsafe {
                if up { ep.VolumeStepUp(&guid).is_ok() } else { ep.VolumeStepDown(&guid).is_ok() }
            }
        })
        .unwrap_or(false)
    }

    pub fn get_mute() -> bool {
        with_endpoint(|ep| unsafe { ep.GetMute() }.map(|m| m.as_bool()).unwrap_or(false))
            .unwrap_or(false)
    }

    pub fn set_mute(mute: bool) -> bool {
        with_endpoint(|ep| {
            let guid = windows::core::GUID::zeroed();
            unsafe { ep.SetMute(mute, &guid).is_ok() }
        })
        .unwrap_or(false)
    }

    pub fn set_volume(level: f32) {
        with_endpoint(|ep| {
            let guid = windows::core::GUID::zeroed();
            unsafe { ep.SetMasterVolumeLevelScalar(level, &guid).is_ok() }
        });
    }
}

#[cfg(target_os = "windows")]
pub fn get_mute() -> bool {
    cached_volume::get_mute()
}

#[cfg(target_os = "windows")]
pub fn get_volume() -> f32 {
    cached_volume::read_volume().unwrap_or_else(get_volume_legacy)
}

#[cfg(target_os = "windows")]
pub fn set_volume(level: f32) {
    cached_volume::set_volume(level.clamp(0.0, 1.0));
}

#[cfg(target_os = "windows")]
fn get_volume_legacy() -> f32 {
    unsafe {
        let mut vol: u32 = 0;
        waveOutGetVolume(0, &mut vol);
        (vol & 0xFFFF) as f32 / 0xFFFF as f32
    }
}

// ---------------------------------------------------------------------------
// Media playing – runs on a background thread to avoid blocking the UI
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
static MEDIA_PLAYING: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "windows")]
pub fn media_is_playing() -> bool {
    MEDIA_PLAYING.load(Ordering::Relaxed)
}

#[cfg(target_os = "windows")]
pub fn start_media_poller() {
    std::thread::Builder::new()
        .name("media-poller".into())
        .spawn(|| {
            let mut last_track: Option<(String, String)> = None;
            loop {
                let playing = poll_media_playing(&mut last_track);
                MEDIA_PLAYING.store(playing, Ordering::Relaxed);
                if !playing {
                    *MEDIA_TEXT.lock().unwrap_or_else(|p| p.into_inner()) = None;
                    last_track = None;
                }
                std::thread::sleep(std::time::Duration::from_millis(200));
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn poll_media_playing(last_track: &mut Option<(String, String)>) -> bool {
    use windows::Media::Control::{
        GlobalSystemMediaTransportControlsSessionManager,
        GlobalSystemMediaTransportControlsSessionPlaybackStatus,
    };

    let manager = match GlobalSystemMediaTransportControlsSessionManager::RequestAsync() {
        Ok(op) => match op.get() {
            Ok(manager) => manager,
            Err(_) => return false,
        },
        Err(_) => return false,
    };

    let session = match manager.GetCurrentSession() {
        Ok(session) => session,
        Err(_) => return false,
    };

    let playback_info = match session.GetPlaybackInfo() {
        Ok(info) => info,
        Err(_) => return false,
    };

    let playing = matches!(
        playback_info.PlaybackStatus(),
        Ok(GlobalSystemMediaTransportControlsSessionPlaybackStatus::Playing)
    );

    if playing {
        fetch_media_properties(&session, last_track);
    }

    playing
}

// ---------------------------------------------------------------------------
// Audio spectrum – WASAPI loopback capture + FFT on a background thread
// ---------------------------------------------------------------------------

pub const AUDIO_BANDS: usize = 4;

#[cfg(target_os = "windows")]
static AUDIO_BAND_LEVELS: [AtomicU32; AUDIO_BANDS] =
    [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];

#[cfg(target_os = "windows")]
pub fn audio_bands() -> [f32; AUDIO_BANDS] {
    let mut out = [0.0f32; AUDIO_BANDS];
    for (i, band) in out.iter_mut().enumerate() {
        *band = AUDIO_BAND_LEVELS[i].load(Ordering::Relaxed) as f32 / 4096.0;
    }
    out
}

/// Captures whatever is playing through the default output device and stores a
/// 4-band spectrum ([f32; 4], 0..1) for the pill visualizer.
#[cfg(target_os = "windows")]
pub fn start_audio_spectrum_poller() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};

    std::thread::Builder::new()
        .name("audio-spectrum".into())
        .spawn(|| {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let mut smooth = [0.0f32; AUDIO_BANDS];
            loop {
                match unsafe { run_loopback_analyzer(&mut smooth) } {
                    Ok(_) => {}
                    Err(_) => {
                        for level in smooth.iter_mut() {
                            *level = 0.0;
                        }
                        store_bands(&smooth);
                        std::thread::sleep(std::time::Duration::from_millis(500));
                    }
                }
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn store_bands(bands: &[f32; AUDIO_BANDS]) {
    for (i, v) in bands.iter().enumerate() {
        let scaled = (v.clamp(0.0, 1.0) * 4096.0).round() as u32;
        AUDIO_BAND_LEVELS[i].store(scaled, Ordering::Relaxed);
    }
}

const FFT_SIZE: usize = 1024;

#[cfg(target_os = "windows")]
unsafe fn run_loopback_analyzer(smooth: &mut [f32; AUDIO_BANDS]) -> windows::core::Result<()> {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    unsafe {
    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    // Remember the endpoint so we can reconnect when it changes (e.g. Bluetooth).
    let initial_endpoint_id = {
        let id = device.GetId()?;
        let s = id.to_string()?;
        CoTaskMemFree(Some(id.as_ptr().cast()));
        s
    };
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
        let mix_format = client.GetMixFormat()?;

        // WAVEFORMATEX is repr(packed) – read fields through the raw pointer only.
        let format_tag = (*mix_format).wFormatTag;
        let cb_size = (*mix_format).cbSize;
        let channels = ((*mix_format).nChannels as usize).max(1);

        // Classify the mix format so we know how to decode samples.
        let sample_format_is_float = if format_tag == 3 {
            // WAVE_FORMAT_IEEE_FLOAT
            true
        } else if format_tag == 0xFFFE && cb_size as usize >= 22 {
            // WAVE_FORMAT_EXTENSIBLE – inspect the SubFormat GUID
            const IEEE_FLOAT_SUBFORMAT: windows::core::GUID =
                windows::core::GUID::from_u128(0x0000_0003_0000_0010_8000_00AA_0038_9B71);
            let ext = mix_format as *const WAVEFORMATEXTENSIBLE;
            let sub_format: windows::core::GUID =
                std::ptr::addr_of!((*ext).SubFormat).read_unaligned();
            sub_format == IEEE_FLOAT_SUBFORMAT
        } else {
            false
        };

        client.Initialize(
            AUDCLNT_SHAREMODE_SHARED,
            AUDCLNT_STREAMFLAGS_LOOPBACK,
            0,
            0,
            mix_format,
            None,
        )?;
        let capture: IAudioCaptureClient = client.GetService()?;
        client.Start()?;
        CoTaskMemFree(Some(mix_format.cast()));

        let mut window = vec![0.0f32; FFT_SIZE];
        let mut write_pos = 0usize;
        let mut filled = 0usize;
        let mut ticks: u64 = 0;

        loop {
            std::thread::sleep(std::time::Duration::from_millis(10));
            ticks += 1;

            // Every ~2s: if the default output device changed, bail so the
            // outer loop reconnects to the new endpoint (old loopback runs silent).
            if ticks % 200 == 0 {
                let current = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
                let id = current.GetId()?;
                let id_str = id.to_string()?;
                CoTaskMemFree(Some(id.as_ptr().cast()));
                if id_str != initial_endpoint_id {
                    return Err(windows::core::Error::from_hresult(windows::core::HRESULT(
                        -2147024882,
                    )));
                }
            }

            let mut packet = capture.GetNextPacketSize()?;
            while packet > 0 {
                let mut data: *mut u8 = std::ptr::null_mut();
                let mut frames: u32 = 0;
                let mut flags: u32 = 0;
                capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;

                if frames > 0 && flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 == 0 {
                    let bytes_per_frame = channels * if sample_format_is_float { 4 } else { 2 };
                    let slice = std::slice::from_raw_parts(
                        data,
                        frames as usize * bytes_per_frame,
                    );
                    for frame in 0..frames as usize {
                        let base = frame * bytes_per_frame;
                        let mut mono = 0.0f32;
                        for ch in 0..channels {
                            let s = if sample_format_is_float {
                                f32::from_le_bytes([
                                    slice[base + ch * 4],
                                    slice[base + ch * 4 + 1],
                                    slice[base + ch * 4 + 2],
                                    slice[base + ch * 4 + 3],
                                ])
                            } else {
                                i16::from_le_bytes([
                                    slice[base + ch * 2],
                                    slice[base + ch * 2 + 1],
                                ]) as f32
                                    / 32768.0
                            };
                            mono += s;
                        }
                        mono /= channels as f32;
                        window[write_pos] = mono;
                        write_pos = (write_pos + 1) % FFT_SIZE;
                        filled = filled.min(FFT_SIZE - 1) + 1;
                    }
                } else if frames > 0 {
                    for _ in 0..frames {
                        window[write_pos] = 0.0;
                        write_pos = (write_pos + 1) % FFT_SIZE;
                        filled = filled.min(FFT_SIZE - 1) + 1;
                    }
                }

                let _ = capture.ReleaseBuffer(frames);
                packet = capture.GetNextPacketSize()?;
            }

            if filled >= FFT_SIZE {
                // Reassemble the ring buffer in chronological order.
                let mut ordered = vec![0.0f32; FFT_SIZE];
                ordered[..FFT_SIZE - write_pos].copy_from_slice(&window[write_pos..]);
                ordered[FFT_SIZE - write_pos..].copy_from_slice(&window[..write_pos]);

                let mut target = [0.0f32; AUDIO_BANDS];
                compute_spectrum(&ordered, &mut target);

                // Fast attack, slow release.
                for i in 0..AUDIO_BANDS {
                    let factor = if target[i] > smooth[i] { 0.55 } else { 0.18 };
                    smooth[i] += (target[i] - smooth[i]) * factor;
                }
                store_bands(smooth);
            }
        }
    }
}

fn compute_spectrum(samples: &[f32], bands: &mut [f32; AUDIO_BANDS]) {
    let n = samples.len();
    if !n.is_power_of_two() || n < 64 {
        return;
    }

    // Hann window
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, s)| {
            let w = 0.5 * (1.0 - (std::f32::consts::TAU * i as f32 / n as f32).cos());
            s * w
        })
        .collect();
    let mut im = vec![0.0f32; n];

    // Bit-reversal permutation
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j |= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }

    // Cooley-Tukey butterflies
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let ang = -std::f32::consts::TAU / len as f32;
        let mut k = 0;
        while k < n {
            for m in 0..half {
                let wr = (ang * m as f32).cos();
                let wi = (ang * m as f32).sin();
                let a = k + m;
                let b = a + half;
                let tr = re[b] * wr - im[b] * wi;
                let ti = re[b] * wi + im[b] * wr;
                let (ar, ai) = (re[a], im[a]);
                re[a] = ar + tr;
                im[a] = ai + ti;
                re[b] = ar - tr;
                im[b] = ai - ti;
            }
            k += len;
        }
        len <<= 1;
    }

    // Log-spaced band edges across bins [min_bin, n/2]
    let min_bin = 2usize;
    let max_bin = n / 2;
    let ratio = (max_bin / min_bin) as f32;
    let mut edges = [0usize; AUDIO_BANDS + 1];
    for (k, edge) in edges.iter_mut().enumerate() {
        *edge = ((min_bin as f32) * ratio.powf(k as f32 / AUDIO_BANDS as f32)).round() as usize;
    }

    // High frequencies are quieter in music — tilt each band up progressively.
    let tilt_db = [0.0f32, 3.0, 6.0, 9.0];

    for b in 0..AUDIO_BANDS {
        let lo = edges[b].max(1);
        let hi = edges[b + 1].clamp(lo + 1, max_bin);
        let mut sum = 0.0f32;
        for bin in lo..hi.min(max_bin) {
            let mag = (re[bin] * re[bin] + im[bin] * im[bin]).sqrt();
            sum += mag;
        }
        let count = (hi - lo).max(1) as f32;
        // Normalize to approximate amplitude of the loudest component.
        let amp = (sum / count) * (4.0 / n as f32);
        let db = 20.0 * amp.max(1e-7).log10();
        // Map [-66 dB .. -20 dB] → [0 .. 1]
        bands[b] = ((db + tilt_db[b] + 66.0) / 46.0).clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------------------
// Media keys – low-level keyboard hook so volume keys work without explorer
// ---------------------------------------------------------------------------

pub const MEDIA_KEY_NONE: u32 = 0;
pub const MEDIA_KEY_UP: u32 = 1;
pub const MEDIA_KEY_DOWN: u32 = 2;
pub const MEDIA_KEY_MUTE: u32 = 3;

#[cfg(target_os = "windows")]
static MEDIA_KEY_KIND: AtomicU32 = AtomicU32::new(MEDIA_KEY_NONE);
#[cfg(target_os = "windows")]
static MEDIA_KEY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static SUPER_KEY_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static CONTROL_OPEN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static WIN_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "windows")]
static WIN_DOWN_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static ALT_DOWN: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);
#[cfg(target_os = "windows")]
static SWITCHER_TAB_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static ALT_RELEASED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static ESC_PRESSED_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// Millis since boot (matches GetTickCount64 stamps from the hook thread).
#[cfg(target_os = "windows")]
pub fn tick_ms() -> u64 {
    unsafe { GetTickCount64() }
}

/// True if the Super/Windows key (left or right) was pressed within `max_age_ms`.
#[cfg(target_os = "windows")]
pub fn take_super_key(max_age_ms: u64) -> bool {
    let stamp = SUPER_KEY_MS.load(Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        SUPER_KEY_MS.store(0, Ordering::Relaxed);
        return false;
    }
    SUPER_KEY_MS.store(0, Ordering::Relaxed);
    true
}

/// True if the Control Center shortcut (Win+A) was pressed within `max_age_ms`.
#[cfg(target_os = "windows")]
pub fn take_control_open(max_age_ms: u64) -> bool {
    let stamp = CONTROL_OPEN_MS.load(Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        CONTROL_OPEN_MS.store(0, Ordering::Relaxed);
        return false;
    }
    CONTROL_OPEN_MS.store(0, Ordering::Relaxed);
    true
}

/// True if the Alt key is currently held (drives the Alt+Tab switcher lifecycle).
#[cfg(target_os = "windows")]
pub fn switcher_alt_down() -> bool {
    ALT_DOWN.load(Ordering::Relaxed)
}

/// Timestamp of the most recent Tab-tap while Alt is held, or 0 if none.
#[cfg(target_os = "windows")]
pub fn switcher_tab_ms() -> u64 {
    SWITCHER_TAB_MS.load(Ordering::Relaxed)
}

/// Consumes the Alt-released event, firing once when Alt is let go.
#[cfg(target_os = "windows")]
pub fn take_alt_released(max_age_ms: u64) -> bool {
    let stamp = ALT_RELEASED_MS.load(Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        ALT_RELEASED_MS.store(0, Ordering::Relaxed);
        return false;
    }
    ALT_RELEASED_MS.store(0, Ordering::Relaxed);
    true
}

/// True if the Escape key was pressed within `max_age_ms` (consumes the event),
/// so overlays close even when the window isn't focused.
#[cfg(target_os = "windows")]
pub fn take_escape(max_age_ms: u64) -> bool {
    let stamp = ESC_PRESSED_MS.load(Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        ESC_PRESSED_MS.store(0, Ordering::Relaxed);
        return false;
    }
    ESC_PRESSED_MS.store(0, Ordering::Relaxed);
    true
}

/// Most recent media-key event, if it happened within `max_age_ms`.
#[cfg(target_os = "windows")]
pub fn take_media_key_event(max_age_ms: u64) -> Option<u32> {
    let kind = MEDIA_KEY_KIND.load(Ordering::Relaxed);
    if kind == MEDIA_KEY_NONE {
        return None;
    }
    let stamp = MEDIA_KEY_MS.load(Ordering::Relaxed);
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        MEDIA_KEY_KIND.store(MEDIA_KEY_NONE, Ordering::Relaxed);
        return None;
    }
    // Consume so a single press doesn't retrigger.
    MEDIA_KEY_KIND.store(MEDIA_KEY_NONE, Ordering::Relaxed);
    Some(kind)
}

#[cfg(target_os = "windows")]
fn shell_is_running() -> bool {
    use windows::Win32::UI::WindowsAndMessaging::GetShellWindow;
    unsafe { !GetShellWindow().is_invalid() }
}

#[cfg(target_os = "windows")]
fn apply_media_key(kind: u32) -> bool {
    match kind {
        MEDIA_KEY_UP => cached_volume::step_volume(true),
        MEDIA_KEY_DOWN => cached_volume::step_volume(false),
        MEDIA_KEY_MUTE => cached_volume::set_mute(!cached_volume::get_mute()),
        _ => false,
    }
}

#[cfg(target_os = "windows")]
pub fn start_media_key_hook() {
    use windows::Win32::Foundation::{HINSTANCE, LRESULT, WPARAM, LPARAM};
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, GetMessageW, MSG, SetWindowsHookExW, HOOKPROC,
        KBDLLHOOKSTRUCT, LLKHF_INJECTED, WH_KEYBOARD_LL,
    };
    const WM_KEYDOWN: usize = 0x0100;
    const WM_SYSKEYDOWN: usize = 0x0104;
    const WM_KEYUP: usize = 0x0101;
    const WM_SYSKEYUP: usize = 0x0105;
    const VK_VOLUME_UP: u32 = 0xAF;
    const VK_VOLUME_DOWN: u32 = 0xAE;
    const VK_VOLUME_MUTE: u32 = 0xAD;
    const VK_LWIN: u32 = 0x5B;
    const VK_RWIN: u32 = 0x5C;
    const VK_A: u32 = 0x41;
    const VK_LMENU: u32 = 0xA4;
    const VK_RMENU: u32 = 0xA5;
    const VK_MENU: u32 = 0x12;
    const VK_TAB: u32 = 0x09;
    const VK_ESCAPE: u32 = 0x1B;


    unsafe extern "system" fn ll_key_proc(
        code: i32,
        wparam: WPARAM,
        lparam: LPARAM,
    ) -> LRESULT {
        unsafe {
            if code >= 0 {
                let kb = &*(lparam.0 as *const KBDLLHOOKSTRUCT);
                // Only real (non-injected) presses.
                let real = (kb.flags & LLKHF_INJECTED).0 == 0;
                if real {
                    // Track the Super/Windows key state for Win+<key> combos, and
                    // swallow the Win key itself so the shell's Start menu doesn't
                    // steal focus (keeps our launcher from insta-closing).
                    if kb.vkCode == VK_LWIN || kb.vkCode == VK_RWIN {
                        if wparam.0 == WM_KEYDOWN || wparam.0 == WM_SYSKEYDOWN {
                            WIN_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);
                            WIN_DOWN_MS.store(tick_ms(), std::sync::atomic::Ordering::Relaxed);
                            SUPER_KEY_MS.store(tick_ms(), Ordering::Relaxed);
                        } else if wparam.0 == WM_KEYUP || wparam.0 == WM_SYSKEYUP {
                            WIN_DOWN.store(false, std::sync::atomic::Ordering::Relaxed);
                        }
                        return LRESULT(1); // exclusive — no native Start menu
                    }

                    // Alt+Tab switcher: track Alt on both normal & sys messages
                    // (Alt is delivered to apps as WM_SYS*), and swallow Tab so
                    // the default switcher never appears.
                    let is_alt = kb.vkCode == VK_LMENU || kb.vkCode == VK_RMENU || kb.vkCode == VK_MENU;
                    if is_alt {
                        if wparam.0 == WM_KEYDOWN || wparam.0 == WM_SYSKEYDOWN {
                            ALT_DOWN.store(true, std::sync::atomic::Ordering::Relaxed);
                        } else if wparam.0 == WM_KEYUP || wparam.0 == WM_SYSKEYUP {
                            ALT_DOWN.store(false, std::sync::atomic::Ordering::Relaxed);
                            ALT_RELEASED_MS.store(tick_ms(), Ordering::Relaxed);
                        }
                    }
                    let is_tab_key = wparam.0 == WM_KEYDOWN || wparam.0 == WM_SYSKEYDOWN;
                    let is_tab_up = wparam.0 == WM_KEYUP || wparam.0 == WM_SYSKEYUP;
                    if (is_tab_key || is_tab_up)
                        && kb.vkCode == VK_TAB
                        && ALT_DOWN.load(std::sync::atomic::Ordering::Relaxed)
                    {
                        if is_tab_key {
                            SWITCHER_TAB_MS.store(tick_ms(), Ordering::Relaxed);
                        }
                        return LRESULT(1); // swallow — we handle Alt+Tab ourselves
                    }

                    // Global Escape detection — closes overlays even when another
                    // app has focus.
                    if (wparam.0 == WM_KEYDOWN || wparam.0 == WM_SYSKEYDOWN)
                        && kb.vkCode == VK_ESCAPE
                    {
                        ESC_PRESSED_MS.store(tick_ms(), Ordering::Relaxed);
                    }

                    if wparam.0 == WM_KEYDOWN || wparam.0 == WM_SYSKEYDOWN {
                        // Win+A → open the control center. Only fire if the 'A'
                        // arrives within a short window of the Win press so a bare
                        // Win keypress can't combine with a later 'a' keystroke.
                        let win_was_recent = WIN_DOWN.load(std::sync::atomic::Ordering::Relaxed)
                            && tick_ms().saturating_sub(WIN_DOWN_MS.load(std::sync::atomic::Ordering::Relaxed)) < 500;
                        if kb.vkCode == VK_A && win_was_recent {
                            CONTROL_OPEN_MS.store(tick_ms(), Ordering::Relaxed);
                            return LRESULT(1);
                        }
                        let kind = match kb.vkCode {
                            VK_VOLUME_UP => MEDIA_KEY_UP,
                            VK_VOLUME_DOWN => MEDIA_KEY_DOWN,
                            VK_VOLUME_MUTE => MEDIA_KEY_MUTE,
                            _ => MEDIA_KEY_NONE,
                        };
                        if kind != MEDIA_KEY_NONE {
                            MEDIA_KEY_KIND.store(kind, Ordering::Relaxed);
                            MEDIA_KEY_MS.store(tick_ms(), Ordering::Relaxed);
                            if !shell_is_running() {
                                // No shell to handle the key — apply it ourselves and swallow it.
                                apply_media_key(kind);
                                return LRESULT(1);
                            }
                        }
                    }
                }
            }
            CallNextHookEx(None, code, wparam, lparam)
        }
    }

    std::thread::Builder::new()
        .name("media-keys".into())
        .spawn(move || unsafe {
            let hook_proc: HOOKPROC = Some(ll_key_proc);
            let hook = SetWindowsHookExW(WH_KEYBOARD_LL, hook_proc, HINSTANCE::default(), 0);
            if hook.is_err() {
                return;
            }
            let mut msg = MSG::default();
            while GetMessageW(&mut msg, None, 0, 0).as_bool() {}
        })
        .ok();
}

// ---------------------------------------------------------------------------
// Session actions – Lock / Suspend / Log Off / Reboot / Power Off
// ---------------------------------------------------------------------------

pub const POWER_ACTION_LOCK: u32 = 0;
pub const POWER_ACTION_SUSPEND: u32 = 1;
pub const POWER_ACTION_LOGOUT: u32 = 2;
pub const POWER_ACTION_REBOOT: u32 = 3;
pub const POWER_ACTION_SHUTDOWN: u32 = 4;

#[cfg(target_os = "windows")]
pub fn perform_power_action(action: u32) -> bool {
    use windows::Win32::System::Power::SetSuspendState;
    use windows::Win32::System::Shutdown::{
        ExitWindowsEx, LockWorkStation, EWX_LOGOFF, EWX_POWEROFF, EWX_REBOOT,
        SHUTDOWN_REASON,
    };

    unsafe {
        let reason = SHUTDOWN_REASON(0);
        match action {
            POWER_ACTION_LOCK => LockWorkStation().is_ok(),
            POWER_ACTION_SUSPEND => SetSuspendState(false, true, false).as_bool(),
            POWER_ACTION_LOGOUT => ExitWindowsEx(EWX_LOGOFF, reason).is_ok(),
            POWER_ACTION_REBOOT => ExitWindowsEx(EWX_REBOOT, reason).is_ok(),
            POWER_ACTION_SHUTDOWN => ExitWindowsEx(EWX_POWEROFF, reason).is_ok(),
            _ => false,
        }
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// Notifications – read toast notifications via UserNotificationListener
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct NotificationInfo {
    pub id: u32,
    pub app: String,
    pub title: String,
    pub body: String,
}

#[cfg(target_os = "windows")]
static NOTIFICATIONS: std::sync::Mutex<Vec<NotificationInfo>> =
    std::sync::Mutex::new(Vec::new());
#[cfg(target_os = "windows")]
static NEW_NOTIFICATION_MS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
#[cfg(target_os = "windows")]
static LATEST_NOTIF: std::sync::Mutex<Option<NotificationInfo>> = std::sync::Mutex::new(None);

#[cfg(target_os = "windows")]
pub fn take_latest_notification() -> Option<NotificationInfo> {
    LATEST_NOTIF.lock().ok().and_then(|mut g| g.take())
}

/// Timestamp of the most recent "new" notification (a previously-unseen id),
/// if one arrived within `max_age_ms`. Consumes the event once.
#[cfg(target_os = "windows")]
pub fn take_new_notification(max_age_ms: u64) -> bool {
    let stamp = NEW_NOTIFICATION_MS.load(Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        NEW_NOTIFICATION_MS.store(0, Ordering::Relaxed);
        return false;
    }
    NEW_NOTIFICATION_MS.store(0, Ordering::Relaxed);
    true
}

/// Polls Windows toast notifications on a background thread and flags new ones.
#[cfg(target_os = "windows")]
pub fn start_notification_listener() {
    use windows::UI::Notifications::Management::UserNotificationListener;
    use windows::UI::Notifications::NotificationKinds;
    use windows::Foundation::Collections::IVectorView;

    std::thread::Builder::new()
        .name("notifications".into())
        .spawn(move || {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            let listener = match UserNotificationListener::Current() {
                Ok(l) => l,
                Err(e) => {
                    eprintln!("[notify] UserNotificationListener::Current failed: {e:?}");
                    return;
                }
            };

            // Request access (needed to read notifications).
            match listener.RequestAccessAsync().and_then(|op| op.get()) {
                Ok(status) => eprintln!("[notify] access status = {}", status.0),
                Err(e) => eprintln!("[notify] RequestAccessAsync failed: {e:?}"),
            }

            let mut known: std::collections::HashSet<u32> = std::collections::HashSet::new();
            loop {
                match listener.GetNotificationsAsync(NotificationKinds(1)).and_then(|op| op.get()) {
                    Ok(view) => {
                        let list: IVectorView<windows::UI::Notifications::UserNotification> = view;
                        let count = list.Size().unwrap_or(0);
                        let mut snapshot = Vec::new();
                        for i in 0..count {
                            if let Ok(un) = list.GetAt(i) {
                                let info = extract_notification(&un);
                                let id = info.id;
                                if !known.contains(&id) {
                                    known.insert(id);
                                    NEW_NOTIFICATION_MS.store(tick_ms(), Ordering::Relaxed);
                                    // Stash this specific notification so the UI can
                                    // read it without racing the stale full list.
                                    if let Ok(mut slot) = LATEST_NOTIF.lock() {
                                        *slot = Some(info.clone());
                                    }
                                }
                                if !info.title.is_empty() || !info.body.is_empty() {
                                    snapshot.push(info);
                                }
                            }
                        }
                        if let Ok(mut g) = NOTIFICATIONS.lock() {
                            *g = snapshot;
                        }
                    }
                    Err(e) => eprintln!("[notify] GetNotificationsAsync failed: {e:?}"),
                }
                std::thread::sleep(std::time::Duration::from_millis(1000));
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn extract_notification(
    un: &windows::UI::Notifications::UserNotification,
) -> NotificationInfo {
    let id = un.Id().unwrap_or(0);
    let app = un
        .AppInfo()
        .ok()
        .and_then(|a| a.DisplayInfo().ok())
        .and_then(|d| d.DisplayName().ok())
        .map(|s| s.to_string())
        .unwrap_or_default();
    let (title, body) = notification_text(un);
    NotificationInfo { id, app, title, body }
}

#[cfg(target_os = "windows")]
fn notification_text(un: &windows::UI::Notifications::UserNotification) -> (String, String) {
    use windows::UI::Notifications::KnownNotificationBindings;
    let Ok(notif) = un.Notification() else { return (String::new(), String::new()) };
    let Ok(visual) = notif.Visual() else { return (String::new(), String::new()) };
    let Ok(template) = KnownNotificationBindings::ToastGeneric() else { return (String::new(), String::new()) };
    let Ok(binding) = visual.GetBinding(&template) else { return (String::new(), String::new()) };
    let Ok(texts) = binding.GetTextElements() else { return (String::new(), String::new()) };
    let count = texts.Size().unwrap_or(0);
    let mut strings = Vec::new();
    for i in 0..count {
        if let Ok(t) = texts.GetAt(i) {
            if let Ok(s) = t.Text() {
                let s = s.to_string();
                if !s.is_empty() {
                    strings.push(s);
                }
            }
        }
    }
    let title = strings.first().cloned().unwrap_or_default();
    let body = strings.get(1).cloned().unwrap_or_default();
    (title, body)
}

// ---------------------------------------------------------------------------
// App icons – HICON → RGBA pixels for the launcher
// ---------------------------------------------------------------------------

pub struct AppIconPixels {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>, // straight alpha
}

#[cfg(target_os = "windows")]
pub fn extract_app_icon(path: &Path) -> Option<AppIconPixels> {
    // High-resolution path: SHIL_JUMBO (256px) via the shell image list. This
    // is crisp when downscaled AND doesn't include the shortcut-arrow overlay
    // that SHGetFileInfoW bakes into .lnk icons.
    if let Some(px) = extract_icon_via_jumbo(path) {
        return Some(px);
    }
    extract_icon_via_fileinfo(path)
}

#[cfg(target_os = "windows")]
fn extract_icon_via_jumbo(path: &Path) -> Option<AppIconPixels> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Controls::IImageList;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGetImageList, SHGFI_ICON, SHGFI_SYSICONINDEX, SHIL_JUMBO};
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    unsafe {
        // SHGetFileInfoW on a .lnk resolves the target's system icon index;
        // SHGFI_SYSICONINDEX lets us then fetch the full-res icon from the list.
        let display = PCWSTR(wide(path.as_os_str()).as_ptr());
        let mut sfi = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            display,
            Default::default(),
            Some(&mut sfi as *mut _ as *mut _),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_SYSICONINDEX,
        );
        if ok == 0 {
            return None;
        }
        let icon_index = sfi.iIcon;
        let _ = DestroyIcon(sfi.hIcon);

        let list: IImageList = SHGetImageList(SHIL_JUMBO as i32).ok()?;
        let hicon: HICON = list.GetIcon(icon_index, 0).ok()?;
        let px = pixels_from_hicon(hicon)?;
        let _ = DestroyIcon(hicon);
        Some(px)
    }
}

#[cfg(target_os = "windows")]
fn extract_icon_via_fileinfo(path: &Path) -> Option<AppIconPixels> {
    use windows::core::PCWSTR;
    use windows::Win32::UI::Shell::{SHGetFileInfoW, SHFILEINFOW, SHGFI_ICON, SHGFI_LARGEICON};
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    unsafe {
        let wide = wide(path.as_os_str());
        let mut sfi = SHFILEINFOW::default();
        let ok = SHGetFileInfoW(
            PCWSTR(wide.as_ptr()),
            Default::default(),
            Some(&mut sfi as *mut _ as *mut _),
            std::mem::size_of::<SHFILEINFOW>() as u32,
            SHGFI_ICON | SHGFI_LARGEICON,
        );
        if ok == 0 || sfi.hIcon.is_invalid() {
            return None;
        }
        let px = pixels_from_hicon(sfi.hIcon);
        let _ = DestroyIcon(sfi.hIcon);
        px
    }
}

/// Extracts a window's icon (large) into RGBA pixels, falling back to the
/// window class icon. Used by the Alt+Tab switcher thumbnails.
#[cfg(target_os = "windows")]
pub fn extract_app_icon_for_window(hwnd_isize: isize) -> Option<AppIconPixels> {
    #[cfg(target_os = "windows")]
    use windows::Win32::UI::WindowsAndMessaging::{
        GetClassLongPtrW, SendMessageW, GCLP_HICON, ICON_BIG, WM_GETICON,
    };

    unsafe {
        let hwnd = windows::Win32::Foundation::HWND(hwnd_isize as *mut core::ffi::c_void);
        // Try the window icon first, then the class icon.
        let lr = SendMessageW(
            hwnd,
            WM_GETICON,
            windows::Win32::Foundation::WPARAM(ICON_BIG as usize),
            windows::Win32::Foundation::LPARAM(0),
        );
        let mut icon = windows::Win32::UI::WindowsAndMessaging::HICON(lr.0 as *mut core::ffi::c_void);
        if icon.is_invalid() {
            icon = windows::Win32::UI::WindowsAndMessaging::HICON(
                GetClassLongPtrW(hwnd, GCLP_HICON) as *mut core::ffi::c_void,
            );
        }
        if icon.is_invalid() {
            return None;
        }
        pixels_from_hicon(icon)
    }
}

#[cfg(target_os = "windows")]
fn pixels_from_hicon(hicon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<AppIconPixels> {
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW,
        ReleaseDC, SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB,
        DIB_RGB_COLORS,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, GetIconInfo, ICONINFO};

    unsafe {
        let mut info = ICONINFO::default();
        if GetIconInfo(hicon, &mut info).is_err() {
            let _ = DestroyIcon(hicon);
            return None;
        }
        let mut bm = BITMAP::default();
        let got = GetObjectW(
            info.hbmColor,
            std::mem::size_of::<BITMAP>() as i32,
            Some(&mut bm as *mut _ as *mut _),
        );
        if got == 0 || bm.bmWidth == 0 || bm.bmHeight == 0 {
            let _ = DeleteObject(info.hbmColor);
            let _ = DeleteObject(info.hbmMask);
            return None;
        }
        let w = bm.bmWidth as usize;
        let h = bm.bmHeight.unsigned_abs() as usize;

        let hdc_screen = GetDC(None);
        let hdc_mem = CreateCompatibleDC(hdc_screen);
        let old = SelectObject(hdc_mem, info.hbmColor);

        let mut bmi = BITMAPINFO::default();
        bmi.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
        bmi.bmiHeader.biWidth = w as i32;
        bmi.bmiHeader.biHeight = -(h as i32); // top-down
        bmi.bmiHeader.biPlanes = 1;
        bmi.bmiHeader.biBitCount = 32;
        bmi.bmiHeader.biCompression = BI_RGB.0;

        let mut buf = vec![0u8; w * h * 4];
        let lines = GetDIBits(
            hdc_mem,
            info.hbmColor,
            0,
            h as u32,
            Some(buf.as_mut_ptr().cast()),
            &mut bmi,
            DIB_RGB_COLORS,
        );

        SelectObject(hdc_mem, old);
        let _ = DeleteDC(hdc_mem);
        ReleaseDC(None, hdc_screen);
        let _ = DeleteObject(info.hbmColor);
        let _ = DeleteObject(info.hbmMask);

        if lines == 0 {
            return None;
        }

        // BGRA → RGBA, with an alpha heuristic: legacy icons carry garbage
        // alpha; treat all-zero alpha as opaque.
        let mut any_alpha = false;
        for px in buf.chunks_exact_mut(4) {
            px.swap(0, 2);
            if px[3] != 0 {
                any_alpha = true;
            }
        }
        if !any_alpha {
            for px in buf.chunks_exact_mut(4) {
                px[3] = 255;
            }
        }

        Some(AppIconPixels { width: w, height: h, rgba: buf })
    }
}

// ---------------------------------------------------------------------------
// Media metadata – title/artist/album art from SMTC
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct MediaMeta {
    pub title: String,
    pub artist: String,
}
#[derive(Clone)]
pub struct MediaArt {
    pub width: usize,
    pub height: usize,
    pub rgba: Vec<u8>,
}

#[cfg(target_os = "windows")]
static MEDIA_TEXT: std::sync::Mutex<Option<MediaMeta>> = std::sync::Mutex::new(None);
#[cfg(target_os = "windows")]
static MEDIA_ART: std::sync::Mutex<Option<MediaArt>> = std::sync::Mutex::new(None);
#[cfg(target_os = "windows")]
static MEDIA_ART_GEN: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "windows")]
static TRACK_CHANGE_MS: AtomicU64 = AtomicU64::new(0);

/// Records that the now-playing track changed (called by the media poller).
#[cfg(target_os = "windows")]
pub fn mark_track_change() {
    TRACK_CHANGE_MS.store(tick_ms(), std::sync::atomic::Ordering::Relaxed);
}

/// True a track change happened within `max_age_ms`. Consumes the event so it
/// only fires once.
#[cfg(target_os = "windows")]
pub fn take_track_change(max_age_ms: u64) -> bool {
    let stamp = TRACK_CHANGE_MS.load(std::sync::atomic::Ordering::Relaxed);
    if stamp == 0 {
        return false;
    }
    if tick_ms().saturating_sub(stamp) > max_age_ms {
        return false;
    }
    TRACK_CHANGE_MS.store(0, std::sync::atomic::Ordering::Relaxed);
    true
}

/// Title + artist snapshot (cheap clone).
#[cfg(target_os = "windows")]
pub fn media_text() -> Option<MediaMeta> {
    MEDIA_TEXT.lock().ok()?.clone()
}

/// Returns album art pixels only when the generation changed since `last_gen`.
#[cfg(target_os = "windows")]
pub fn media_art_if_new(last_gen: &mut u64) -> Option<MediaArt> {
    let generation = MEDIA_ART_GEN.load(Ordering::Relaxed);
    if generation == 0 || generation == *last_gen {
        return None;
    }
    let art = MEDIA_ART.lock().ok()?.clone()?;
    *last_gen = generation;
    Some(art)
}

#[cfg(not(target_os = "windows"))]
pub fn media_text() -> Option<MediaMeta> { None }
#[cfg(not(target_os = "windows"))]
pub fn media_art_if_new(_last_gen: &mut u64) -> Option<MediaArt> { None }
#[cfg(not(target_os = "windows"))]
pub fn mark_track_change() {}
#[cfg(not(target_os = "windows"))]
pub fn take_track_change(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn system_status() -> SystemStatus { SystemStatus::default() }
#[cfg(not(target_os = "windows"))]
pub fn start_status_poller() {}

#[cfg(not(target_os = "windows"))]
pub fn take_latest_notification() -> Option<NotificationInfo> { None }
#[cfg(not(target_os = "windows"))]
pub fn take_new_notification(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn start_notification_listener() {}

#[cfg(target_os = "windows")]
fn fetch_media_properties(
    session: &windows::Media::Control::GlobalSystemMediaTransportControlsSession,
    last_track: &mut Option<(String, String)>,
) {
    use windows::Storage::Streams::DataReader;

    let props = match session.TryGetMediaPropertiesAsync().and_then(|op| op.get()) {
        Ok(p) => p,
        Err(_) => return,
    };
    let title = props.Title().map(|s| s.to_string()).unwrap_or_default();
    let artist = props.Artist().map(|s| s.to_string()).unwrap_or_default();

    let track = (title.clone(), artist.clone());
    let changed = last_track.as_ref() != Some(&track);
    if !changed {
        return;
    }
    *last_track = Some(track);
    mark_track_change();

    {
        let mut text = MEDIA_TEXT.lock().unwrap_or_else(|p| p.into_inner());
        *text = Some(MediaMeta { title: title.clone(), artist });
    }

    // New track → drop stale art; it will be replaced below on success.
    {
        let mut art = MEDIA_ART.lock().unwrap_or_else(|p| p.into_inner());
        *art = None;
    }

    let Ok(thumb) = props.Thumbnail() else { return };
    let Ok(stream) = thumb.OpenReadAsync().and_then(|op| op.get()) else { return };
    let size = stream.Size().unwrap_or(0);
    if size == 0 || size > 16 * 1024 * 1024 {
        return;
    }
    let Ok(reader) = DataReader::CreateDataReader(&stream) else { return };
    let mut bytes = vec![0u8; size as usize];
    if reader.LoadAsync(size as u32).and_then(|op| op.get()).is_err() {
        return;
    }
    if reader.ReadBytes(&mut bytes).is_err() {
        return;
    }

    if let Ok(img) = image::load_from_memory(&bytes) {
        let rgba = img.to_rgba8();
        let (width, height) = rgba.dimensions();
        let mut art_guard = MEDIA_ART.lock().unwrap_or_else(|p| p.into_inner());
        *art_guard = Some(MediaArt {
            width: width as usize,
            height: height as usize,
            rgba: rgba.into_raw(),
        });
        drop(art_guard);
        MEDIA_ART_GEN.fetch_add(1, Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// System status – battery / Wi-Fi / Bluetooth, polled on a background thread
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy)]
pub struct SystemStatus {
    pub battery_percent: u8, // 0-100
    pub battery_present: bool,
    pub battery_charging: bool,
    pub wifi_connected: bool,
    pub wifi_signal: u8, // 0-100
    pub bluetooth_connected: bool,
    /// 0 = none, 1 = Wi-Fi, 2 = Ethernet (wired LAN).
    pub connection_type: u8,
}

impl Default for SystemStatus {
    fn default() -> Self {
        Self {
            battery_percent: 0,
            battery_present: false,
            battery_charging: false,
            wifi_connected: false,
            wifi_signal: 0,
            bluetooth_connected: false,
            connection_type: 0,
        }
    }
}

#[cfg(target_os = "windows")]
static SYSTEM_STATUS: std::sync::RwLock<SystemStatus> = std::sync::RwLock::new(SystemStatus {
    battery_percent: 0,
    battery_present: false,
    battery_charging: false,
    wifi_connected: false,
    wifi_signal: 0,
    bluetooth_connected: false,
    connection_type: 0,
});

#[cfg(target_os = "windows")]
pub fn system_status() -> SystemStatus {
    *SYSTEM_STATUS.read().unwrap_or_else(|p| p.into_inner())
}

#[cfg(target_os = "windows")]
pub fn start_status_poller() {
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
    std::thread::Builder::new()
        .name("system-status".into())
        .spawn(move || {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            loop {
                let status = poll_system_status();
                *SYSTEM_STATUS.write().unwrap_or_else(|p| p.into_inner()) = status;
                std::thread::sleep(std::time::Duration::from_millis(2000));
            }
        })
        .ok();
}

#[cfg(target_os = "windows")]
fn poll_system_status() -> SystemStatus {
    use windows::Win32::System::Power::{
        GetSystemPowerStatus, SYSTEM_POWER_STATUS,
    };

    let mut status = SystemStatus::default();

    // Battery.
    unsafe {
        let mut spp = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut spp).is_ok() {
            // BatteryFlag 128 = no battery; 255 = unknown.
            let no_battery = spp.BatteryFlag == 128 || spp.BatteryFlag == 255 && spp.BatteryLifePercent == 255;
            if !no_battery {
                status.battery_present = true;
                if spp.BatteryLifePercent != 255 {
                    status.battery_percent = spp.BatteryLifePercent;
                }
                status.battery_charging = spp.ACLineStatus == 1;
            }
        }
    }

    status.wifi_connected = win_check_wifi(&mut status.wifi_signal);
    status.bluetooth_connected = win_check_bluetooth();
    status.connection_type = win_connection_type(status.wifi_connected);

    status
}

/// Determines the active connection type: 2 = Ethernet, 1 = Wi-Fi, 0 = none.
#[cfg(target_os = "windows")]
fn win_connection_type(wifi_connected: bool) -> u8 {
    if wifi_connected {
        return 1;
    }
    // Check for an active wired (Ethernet) adapter.
    use windows::Win32::NetworkManagement::IpHelper::{GetAdaptersInfo, IF_TYPE_IEEE80211, IP_ADAPTER_INFO};
    const ERROR_BUFFER_OVERFLOW: u32 = 111;
    const ERROR_SUCCESS: u32 = 0;
    const IF_TYPE_ETHERNET_CSMACD: u32 = 6;
    unsafe {
        let mut size: u32 = 0;
        let mut rc = GetAdaptersInfo(None, &mut size);
        if rc != ERROR_BUFFER_OVERFLOW {
            return 0;
        }
        let mut buf = vec![0u8; size as usize];
        rc = GetAdaptersInfo(Some(buf.as_mut_ptr().cast::<IP_ADAPTER_INFO>()), &mut size);
        if rc != ERROR_SUCCESS {
            return 0;
        }
        let mut cur = buf.as_ptr().cast::<IP_ADAPTER_INFO>();
        while !cur.is_null() {
            let info = &*cur;
            if info.Type != IF_TYPE_IEEE80211 && info.Type == IF_TYPE_ETHERNET_CSMACD {
                // The adapter is the wired type we care about; treat as Ethernet.
                return 2;
            }
            cur = info.Next;
        }
        0
    }
}

#[cfg(target_os = "windows")]
fn win_check_wifi(signal: &mut u8) -> bool {
    use windows::Win32::NetworkManagement::WiFi::{
        WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle,
        WlanQueryInterface, WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST,
        WLAN_OPCODE_VALUE_TYPE, wlan_intf_opcode_current_connection,
    };
    const WLAN_CLIENT_VERSION: u32 = 2;
    unsafe {
        let mut negotiated: u32 = 0;
        let mut handle = Default::default();
        if WlanOpenHandle(WLAN_CLIENT_VERSION, None, &mut negotiated, &mut handle) != 0 {
            return false;
        }
        let client: windows::Win32::Foundation::HANDLE = handle;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        let rc = WlanEnumInterfaces(client, None, &mut list);
        if rc != 0 || list.is_null() {
            let _ = WlanCloseHandle(client, None);
            return false;
        }

        let iface_list = &*list;
        let mut connected = false;
        if iface_list.dwNumberOfItems > 0 {
            let iface = &iface_list.InterfaceInfo[0];
            let mut data_size: u32 = 0;
            let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
            let mut opcode_type = WLAN_OPCODE_VALUE_TYPE(0);
            let qrc = WlanQueryInterface(
                client,
                &iface.InterfaceGuid,
                wlan_intf_opcode_current_connection,
                None,
                &mut data_size,
                &mut data,
                Some(&mut opcode_type),
            );
            if qrc == 0 && !data.is_null() {
                let attrs: WLAN_CONNECTION_ATTRIBUTES = std::ptr::read_unaligned(data.cast());
                // wlan_interface_state_connected == 1
                if attrs.isState.0 == 1 {
                    connected = true;
                    *signal = attrs.wlanAssociationAttributes.wlanSignalQuality as u8;
                }
            }
            WlanFreeMemory(data);
        }

        WlanFreeMemory(list.cast());
        let _ = WlanCloseHandle(client, None);
        connected
    }
}

#[cfg(target_os = "windows")]
fn win_check_bluetooth() -> bool {
    use windows::Win32::Devices::Bluetooth::*;
    unsafe {
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnConnected: windows::Win32::Foundation::BOOL(1),
            ..Default::default()
        };
        let mut info = BLUETOOTH_DEVICE_INFO::default();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        match BluetoothFindFirstDevice(&params, &mut info) {
            Ok(find) => {
                let connected = info.fConnected.as_bool();
                let _ = BluetoothFindDeviceClose(find);
                connected
            }
            Err(_) => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Device controls – audio endpoints, Wi-Fi, Bluetooth, Do-Not-Disturb
// ---------------------------------------------------------------------------

/// A single audio output device (render default endpoint).
#[derive(Clone)]
pub struct AudioDevice {
    pub id: String,
    pub name: String,
}

/// Enumerates active audio OUTPUT devices (eRender) for the device picker.
#[cfg(target_os = "windows")]
pub fn list_audio_outputs() -> Vec<AudioDevice> {
    use windows::Win32::Media::Audio::{
        eRender, DEVICE_STATE_ACTIVE, IMMDeviceEnumerator, MMDeviceEnumerator,
    };
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(_) => return Vec::new(),
        };
        let collection = match en.EnumAudioEndpoints(eRender, DEVICE_STATE_ACTIVE) {
            Ok(c) => c,
            Err(_) => return Vec::new(),
        };
        let count = collection.GetCount().unwrap_or(0);
        let mut out = Vec::with_capacity(count as usize);
        for i in 0..count {
            let Ok(dev) = collection.Item(i) else { continue };
            let Ok(id) = dev.GetId() else { continue };
            let name = audio_device_friendly_name(&dev);
            out.push(AudioDevice {
                id: id.to_string().unwrap_or_default(),
                name,
            });
        }
        out
    }
}

#[cfg(target_os = "windows")]
fn audio_device_friendly_name(dev: &windows::Win32::Media::Audio::IMMDevice) -> String {
    unsafe {
        let Ok(store) = dev.OpenPropertyStore(windows::Win32::System::Com::STGM_READ) else {
            return String::new();
        };
        // PKEY_Device_FriendlyName
        let key = windows::Win32::UI::Shell::PropertiesSystem::PROPERTYKEY {
            fmtid: windows::core::GUID::from_u128(0xa45c254e_df1c_4efd_8020_67d146a850e0),
            pid: 14,
        };
        let Ok(pv) = store.GetValue(&key) else {
            return String::new();
        };
        // PROPVARIANT: read raw layout (vt:u16 @0, union @8 on 64-bit; VT_LPWSTR==31).
        let raw: *const u8 = std::mem::transmute(pv.as_raw());
        let vt = std::ptr::read_unaligned(raw.cast::<u16>());
        if vt != 31 {
            return String::new();
        }
        let ptr = std::ptr::read_unaligned(raw.add(8).cast::<*mut u16>());
        if ptr.is_null() {
            return String::new();
        }
        let len = (0..).take_while(|&i| *ptr.add(i) != 0).count();
        String::from_utf16_lossy(std::slice::from_raw_parts(ptr, len))
    }
}

/// Sets the default audio output device by its ID (raw IPolicyConfig vtable).
#[cfg(target_os = "windows")]
pub fn set_default_audio_output(id: &str) -> bool {
    use windows::core::Interface;
    use windows::Win32::Media::Audio::{eConsole, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};

    let policy_clsid = windows::core::GUID::from_u128(0x870af99c_171d_4f9e_af0d_e63df40c2bc9);

    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(_) => return false,
        };
        let wide: Vec<u16> = id.encode_utf16().chain(std::iter::once(0)).collect();
        let dev: windows::Win32::Media::Audio::IMMDevice =
            match en.GetDevice(windows::core::PCWSTR(wide.as_ptr())) {
                Ok(d) => d,
                Err(_) => return false,
            };
        let dev_id = match dev.GetId() {
            Ok(pid) => pid,
            Err(_) => return false,
        };

        // CoCreateInstance the policy config as a raw IUnknown, then dispatch
        // SetDefaultEndpoint via its vtable (method index 3).
        let obj: windows::core::IUnknown =
            match CoCreateInstance(&policy_clsid, None, CLSCTX_ALL) {
                Ok(o) => o,
                Err(_) => return false,
            };
        let this = obj.as_raw();
        let vtbl = *(this as *const *const isize);
        let set_default: unsafe extern "system" fn(*mut core::ffi::c_void, *const u16, i32) -> i32 =
            std::mem::transmute(*vtbl.add(3));
        let hr = set_default(this, dev_id.0, eConsole.0);
        hr >= 0
    }
}

/// Toggles the master mute, returning the new state.
#[cfg(target_os = "windows")]
pub fn toggle_mute() -> bool {
    let new = !cached_volume::get_mute();
    cached_volume::set_mute(new);
    new
}

// --- Audio current default ---

/// Returns the friendly name of the current default audio OUTPUT device.
#[cfg(target_os = "windows")]
pub fn default_audio_output_name() -> String {
    use windows::Win32::Media::Audio::{eConsole, eRender, IMMDeviceEnumerator, MMDeviceEnumerator};
    use windows::Win32::System::Com::{CoCreateInstance, CoInitializeEx, CLSCTX_ALL, COINIT_MULTITHREADED};
    unsafe {
        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
        let en: IMMDeviceEnumerator = match CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) {
            Ok(e) => e,
            Err(_) => return String::new(),
        };
        match en.GetDefaultAudioEndpoint(eRender, eConsole) {
            Ok(dev) => audio_device_friendly_name(&dev),
            Err(_) => String::new(),
        }
    }
}

// --- Wi-Fi current SSID ---

/// Returns the SSID of the currently connected Wi-Fi network, if any.
#[cfg(target_os = "windows")]
pub fn current_wifi_ssid() -> String {
    use windows::Win32::NetworkManagement::WiFi::{
        WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanQueryInterface,
        WLAN_CONNECTION_ATTRIBUTES, WLAN_INTERFACE_INFO_LIST, WLAN_OPCODE_VALUE_TYPE,
        wlan_intf_opcode_current_connection,
    };
    const WLAN_CLIENT_VERSION: u32 = 2;
    unsafe {
        let mut negotiated: u32 = 0;
        let mut handle = Default::default();
        if WlanOpenHandle(WLAN_CLIENT_VERSION, None, &mut negotiated, &mut handle) != 0 {
            return String::new();
        }
        let client: windows::Win32::Foundation::HANDLE = handle;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(client, None, &mut list) != 0 || list.is_null() {
            let _ = WlanCloseHandle(client, None);
            return String::new();
        }
        let iface = &(*list).InterfaceInfo[0];
        let mut data_size: u32 = 0;
        let mut data: *mut core::ffi::c_void = std::ptr::null_mut();
        let mut opcode_type = WLAN_OPCODE_VALUE_TYPE(0);
        let rc = WlanQueryInterface(
            client,
            &iface.InterfaceGuid,
            wlan_intf_opcode_current_connection,
            None,
            &mut data_size,
            &mut data,
            Some(&mut opcode_type),
        );
        let mut ssid = String::new();
        if rc == 0 && !data.is_null() {
            let attrs: WLAN_CONNECTION_ATTRIBUTES = std::ptr::read_unaligned(data.cast());
            if attrs.isState.0 == 1 {
                // Profile name (usually equals the SSID).
                ssid = wide_to_string(&attrs.strProfileName);
            }
        }
        WlanFreeMemory(data);
        WlanFreeMemory(list.cast());
        let _ = WlanCloseHandle(client, None);
        ssid
    }
}

// --- Bluetooth current connected ---

/// Returns the name of the first connected Bluetooth device, if any.
#[cfg(target_os = "windows")]
pub fn current_bluetooth_name() -> String {
    list_bluetooth_devices()
        .into_iter()
        .find(|(_, connected)| *connected)
        .map(|(name, _)| name)
        .unwrap_or_default()
}

// --- Wi-Fi ---

#[derive(Clone)]
pub struct WifiNetwork {
    pub ssid: String,
}

/// Returns the list of visible Wi-Fi networks for the first interface.
#[cfg(target_os = "windows")]
pub fn list_wifi_networks() -> Vec<WifiNetwork> {
    use windows::Win32::NetworkManagement::WiFi::{
        WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanGetAvailableNetworkList,
        WlanOpenHandle, WLAN_AVAILABLE_NETWORK_LIST, WLAN_INTERFACE_INFO_LIST,
    };
    const WLAN_CLIENT_VERSION: u32 = 2;
    unsafe {
        let mut negotiated: u32 = 0;
        let mut handle = Default::default();
        if WlanOpenHandle(WLAN_CLIENT_VERSION, None, &mut negotiated, &mut handle) != 0 {
            return Vec::new();
        }
        let client: windows::Win32::Foundation::HANDLE = handle;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(client, None, &mut list) != 0 || list.is_null() {
            let _ = WlanCloseHandle(client, None);
            return Vec::new();
        }
        let iface = &(*list).InterfaceInfo[0];
        let mut avail: *mut WLAN_AVAILABLE_NETWORK_LIST = std::ptr::null_mut();
        let mut out = Vec::new();
        if WlanGetAvailableNetworkList(client, &iface.InterfaceGuid, 0, None, &mut avail) == 0
            && !avail.is_null()
        {
            let net_list = &*avail;
            // `Network` is a flexible array (declared length 1) — index via raw
            // pointer arithmetic to stay in bounds.
            let first = net_list.Network.as_ptr();
            for i in 0..net_list.dwNumberOfItems as usize {
                let net = &*first.add(i);
                let ssid = dot11_ssid_to_string(&net.dot11Ssid);
                if !ssid.is_empty() {
                    out.push(WifiNetwork { ssid });
                }
            }
            WlanFreeMemory(avail.cast());
        }
        WlanFreeMemory(list.cast());
        let _ = WlanCloseHandle(client, None);
        out
    }
}

#[cfg(target_os = "windows")]
pub fn set_wifi_radio(on: bool) -> bool {
    use windows::Win32::NetworkManagement::WiFi::{
        WlanCloseHandle, WlanEnumInterfaces, WlanFreeMemory, WlanOpenHandle, WlanSetInterface,
        WLAN_INTERFACE_INFO_LIST, wlan_intf_opcode_radio_state,
    };
    const WLAN_CLIENT_VERSION: u32 = 2;
    unsafe {
        let mut negotiated: u32 = 0;
        let mut handle = Default::default();
        if WlanOpenHandle(WLAN_CLIENT_VERSION, None, &mut negotiated, &mut handle) != 0 {
            return false;
        }
        let client: windows::Win32::Foundation::HANDLE = handle;
        let mut list: *mut WLAN_INTERFACE_INFO_LIST = std::ptr::null_mut();
        if WlanEnumInterfaces(client, None, &mut list) != 0 || list.is_null() {
            let _ = WlanCloseHandle(client, None);
            return false;
        }
        let iface = &(*list).InterfaceInfo[0];
        // WLAN_RADIO_STATE: one-byte uSoftApHardwareState/uSoftwareRadioState.
        // We only toggle the software state (byte 2 of the 3-byte struct padded to 4).
        let mut radio_state = [0u8; 8];
        // dot11SoftwareRadioState param: set 0/1 (1 = on).
        radio_state[1] = if on { 1 } else { 0 };
        let rc = WlanSetInterface(
            client,
            &iface.InterfaceGuid,
            wlan_intf_opcode_radio_state,
            std::mem::size_of_val(&radio_state) as u32,
            radio_state.as_ptr().cast(),
            None,
        );
        WlanFreeMemory(list.cast());
        let _ = WlanCloseHandle(client, None);
        rc == 0
    }
}

// --- Bluetooth ---

/// Enumerates remembered / currently-connected Bluetooth devices (name + state).
#[cfg(target_os = "windows")]
pub fn list_bluetooth_devices() -> Vec<(String, bool)> {
    use windows::Win32::Devices::Bluetooth::{
        BluetoothFindDeviceClose, BluetoothFindFirstDevice, BluetoothFindNextDevice,
        BLUETOOTH_DEVICE_INFO, BLUETOOTH_DEVICE_SEARCH_PARAMS,
    };
    unsafe {
        let params = BLUETOOTH_DEVICE_SEARCH_PARAMS {
            dwSize: std::mem::size_of::<BLUETOOTH_DEVICE_SEARCH_PARAMS>() as u32,
            fReturnRemembered: windows::Win32::Foundation::BOOL(1),
            fReturnUnknown: windows::Win32::Foundation::BOOL(1),
            fReturnConnected: windows::Win32::Foundation::BOOL(1),
            ..Default::default()
        };
        let mut info = BLUETOOTH_DEVICE_INFO::default();
        info.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
        let mut out = Vec::new();
        let Ok(find) = BluetoothFindFirstDevice(&params, &mut info) else {
            return out;
        };
        loop {
            let name = wide_to_string(&info.szName);
            if !name.is_empty() {
                out.push((name, info.fConnected.as_bool()));
            }
            // Find next.
            let mut next = BLUETOOTH_DEVICE_INFO::default();
            next.dwSize = std::mem::size_of::<BLUETOOTH_DEVICE_INFO>() as u32;
            if BluetoothFindNextDevice(find, &mut next).is_err() {
                break;
            }
            info = next;
        }
        let _ = BluetoothFindDeviceClose(find);
        out
    }
}

/// Disables/re-enables the Bluetooth radio (best effort; bthprops may be absent).
#[allow(dead_code)]
#[cfg(target_os = "windows")]
pub fn set_bluetooth_radio(on: bool) -> bool {
    let _ = on;
    // BluetoothEnableRadio isn't exposed in this crate; return unchanged state.
    false
}

// --- Do Not Disturb (Peace) ---

/// Enables/disables Windows quiet hours (disables notifications).
#[cfg(target_os = "windows")]
pub fn set_quiet_hours(on: bool) {
    let key = r"Software\Microsoft\Windows\CurrentVersion\PushNotifications";
    use windows::Win32::System::Registry::{
        RegCloseKey, RegOpenKeyExW, RegSetValueExW, HKEY_CURRENT_USER, KEY_SET_VALUE, REG_DWORD,
    };
    use windows::core::PCWSTR;
    unsafe {
        let mut hkey = std::mem::zeroed();
        let wide: Vec<u16> = key.encode_utf16().chain(std::iter::once(0)).collect();
        if RegOpenKeyExW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()), 0, KEY_SET_VALUE, &mut hkey)
            .is_ok()
        {
            let val = if on { 0u32 } else { 1u32 };
            let name: Vec<u16> = "ToastEnabled".encode_utf16().chain(std::iter::once(0)).collect();
            let data = val.to_le_bytes();
            let _ = RegSetValueExW(
                hkey,
                PCWSTR(name.as_ptr()),
                0,
                REG_DWORD,
                Some(&data),
            );
            let _ = RegCloseKey(hkey);
        }
    }
}

fn wide_to_string(buf: &[u16]) -> String {
    let len = buf.iter().take_while(|&&c| c != 0).count();
    String::from_utf16_lossy(&buf[..len])
}

#[cfg(target_os = "windows")]
fn dot11_ssid_to_string(ssid: &windows::Win32::NetworkManagement::WiFi::DOT11_SSID) -> String {
    // DOT11_SSID is { uSSIDLength: u32, ucSSID: [u8; 32] } (packed).
    unsafe {
        let p = (ssid as *const _) as *const u8;
        let len = std::ptr::read_unaligned(p.cast::<u32>()) as usize;
        let bytes = std::slice::from_raw_parts(p.add(4), len.min(32));
        String::from_utf8_lossy(bytes).into_owned()
    }
}

// ---------------------------------------------------------------------------
// Alt+Tab - enumerate top-level windows (for the custom app switcher)
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct SwitcherWindow {
    pub hwnd: isize,
    #[allow(dead_code)]
    pub title: String,
}

/// Enumerates visible, titled top-level windows in z-order (topmost first),
/// excluding tool windows — the windows shown by the native Alt+Tab switcher.
#[cfg(target_os = "windows")]
pub fn list_switch_windows() -> Vec<SwitcherWindow> {
    use windows::Win32::UI::WindowsAndMessaging::EnumWindows;

    unsafe {
        extern "system" fn enum_proc(hwnd: windows::Win32::Foundation::HWND, lparam: windows::Win32::Foundation::LPARAM) -> windows::Win32::Foundation::BOOL {
            unsafe {
                let out = &mut *(lparam.0 as *mut Vec<SwitcherWindow>);
                if let Some(w) = switch_window_info(hwnd) {
                    out.push(w);
                }
            }
            windows::Win32::Foundation::BOOL(1)
        }
        let mut out = Vec::new();
        let _ = EnumWindows(Some(enum_proc), windows::Win32::Foundation::LPARAM(&mut out as *mut _ as isize));
        out
    }
}

#[cfg(target_os = "windows")]
fn switch_window_info(hwnd: windows::Win32::Foundation::HWND) -> Option<SwitcherWindow> {
    use windows::Win32::UI::WindowsAndMessaging::{
        GetWindowTextLengthW, GetWindowTextW, GetWindowLongPtrW, IsWindowVisible, GWL_EXSTYLE,
    };

    const WS_EX_TOOLWINDOW: i32 = 0x00000080;
    const WS_EX_APPWINDOW: i32 = 0x00040000;

    unsafe {
        if !IsWindowVisible(hwnd).as_bool() {
            return None;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE) as i32;
        if ex & WS_EX_TOOLWINDOW != 0 && ex & WS_EX_APPWINDOW == 0 {
            return None;
        }
        let len = GetWindowTextLengthW(hwnd);
        if len <= 0 {
            return None;
        }
        let mut buf = vec![0u16; len as usize + 1];
        let read = GetWindowTextW(hwnd, &mut buf);
        if read == 0 {
            return None;
        }
        let title = wide_to_string(&buf[..read as usize]);
        if title.trim().is_empty() {
            return None;
        }
        Some(SwitcherWindow { hwnd: hwnd.0 as isize, title })
    }
}

/// Brings a window to the foreground (and restores it if minimized).
#[cfg(target_os = "windows")]
pub fn activate_switch_window(hwnd_isize: isize) {
    use windows::Win32::UI::WindowsAndMessaging::{
        AllowSetForegroundWindow, BringWindowToTop, SetForegroundWindow, ShowWindow, SW_RESTORE,
        ASFW_ANY,
    };
    unsafe {
        let _ = AllowSetForegroundWindow(ASFW_ANY);
        let hwnd = windows::Win32::Foundation::HWND(hwnd_isize as *mut core::ffi::c_void);
        let _ = ShowWindow(hwnd, SW_RESTORE);

        // Games and other apps often reject SetForegroundWindow from a background
        // process. Attaching our input thread to the target's input thread bypasses
        // the foreground lock so focus is granted reliably.
        let target_thread = windows::Win32::UI::WindowsAndMessaging::GetWindowThreadProcessId(
            hwnd,
            None,
        );
        let our_thread = GetCurrentThreadId();
        let mut attached = false;
        if target_thread != 0 && target_thread != our_thread {
            attached = AttachThreadInput(our_thread, target_thread, 1) != 0;
        }

        let _ = BringWindowToTop(hwnd);
        let _ = SetForegroundWindow(hwnd);

        if attached {
            let _ = AttachThreadInput(our_thread, target_thread, 0);
        }
    }
}

// ---------------------------------------------------------------------------
// App launching
// ---------------------------------------------------------------------------

#[cfg(target_os = "windows")]
pub fn launch_path(path: &Path) {
    let operation = wide("open");
    let file = wide(path.as_os_str());
    unsafe {
        ShellExecuteW(0, operation.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), 1);
    }
}

#[cfg(target_os = "windows")]
fn wide(value: impl AsRef<std::ffi::OsStr>) -> Vec<u16> {
    value.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

// ---------------------------------------------------------------------------
// Non-Windows stubs
// ---------------------------------------------------------------------------

#[cfg(not(target_os = "windows"))]
pub fn set_position(_hwnd: isize, _x: i32, _y: i32) {}
#[cfg(not(target_os = "windows"))]
pub fn resize_window(_hwnd: isize, _width: i32, _height: i32) {}
#[cfg(not(target_os = "windows"))]
pub fn set_position_and_size(_hwnd: isize, _x: i32, _y: i32, _width: i32, _height: i32) {}
#[cfg(not(target_os = "windows"))]
pub fn reset_pos_cache() {}
#[cfg(not(target_os = "windows"))]
pub fn get_screen_width() -> i32 { 1920 }
#[cfg(not(target_os = "windows"))]
pub fn get_screen_height() -> i32 { 1080 }
#[cfg(not(target_os = "windows"))]
pub fn window_scale(_hwnd: isize) -> f32 { 1.0 }
#[cfg(not(target_os = "windows"))]
pub fn foreground_covers_screen() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn foreground_hwnd() -> isize { 0 }
#[cfg(not(target_os = "windows"))]
pub struct FrameCursor { pub screen_x: i32, pub screen_y: i32 }
#[cfg(not(target_os = "windows"))]
pub fn frame_cursor() -> FrameCursor { FrameCursor { screen_x: 0, screen_y: 0 } }
#[cfg(not(target_os = "windows"))]
pub fn cursor_at_top(_cursor: &FrameCursor, _threshold: i32) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn configure_window_styles(_hwnd: isize, _clickthrough: bool, _accepts_focus: bool) {}
#[cfg(not(target_os = "windows"))]
pub fn focus_window(_hwnd: isize) {}
#[cfg(not(target_os = "windows"))]
pub fn get_volume() -> f32 { 0.0 }
#[cfg(not(target_os = "windows"))]
pub fn set_volume(_level: f32) {}
#[cfg(not(target_os = "windows"))]
pub fn media_is_playing() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn start_media_poller() {}

#[cfg(not(target_os = "windows"))]
pub fn tick_ms() -> u64 { 0 }
#[cfg(not(target_os = "windows"))]
pub fn take_media_key_event(_max_age_ms: u64) -> Option<u32> { None }
#[cfg(not(target_os = "windows"))]
pub fn take_super_key(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn take_control_open(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn switcher_alt_down() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn switcher_tab_ms() -> u64 { 0 }
#[cfg(not(target_os = "windows"))]
pub fn take_alt_released(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn take_escape(_max_age_ms: u64) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn list_switch_windows() -> Vec<SwitcherWindow> { Vec::new() }
#[cfg(not(target_os = "windows"))]
pub fn activate_switch_window(_hwnd: isize) {}
#[cfg(not(target_os = "windows"))]
pub fn perform_power_action(_action: u32) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn start_media_key_hook() {}
#[cfg(not(target_os = "windows"))]
pub fn get_mute() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn extract_app_icon(_path: &std::path::Path) -> Option<AppIconPixels> { None }
#[cfg(not(target_os = "windows"))]
pub fn extract_app_icon_for_window(_hwnd: isize) -> Option<AppIconPixels> { None }

#[cfg(not(target_os = "windows"))]
pub const AUDIO_BANDS: usize = 4;

#[cfg(not(target_os = "windows"))]
pub fn audio_bands() -> [f32; AUDIO_BANDS] { [0.0; AUDIO_BANDS] }

#[cfg(not(target_os = "windows"))]
pub fn start_audio_spectrum_poller() {}
#[cfg(not(target_os = "windows"))]
pub fn launch_path(_path: &std::path::Path) {}

#[cfg(not(target_os = "windows"))]
pub fn list_audio_outputs() -> Vec<AudioDevice> { Vec::new() }
#[cfg(not(target_os = "windows"))]
pub fn default_audio_output_name() -> String { String::new() }
#[cfg(not(target_os = "windows"))]
pub fn current_wifi_ssid() -> String { String::new() }
#[cfg(not(target_os = "windows"))]
pub fn current_bluetooth_name() -> String { String::new() }
#[cfg(not(target_os = "windows"))]
pub fn set_default_audio_output(_id: &str) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn toggle_mute() -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn list_wifi_networks() -> Vec<WifiNetwork> { Vec::new() }
#[cfg(not(target_os = "windows"))]
pub fn set_wifi_radio(_on: bool) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn list_bluetooth_devices() -> Vec<(String, bool)> { Vec::new() }
#[cfg(not(target_os = "windows"))]
pub fn set_bluetooth_radio(_on: bool) -> bool { false }
#[cfg(not(target_os = "windows"))]
pub fn set_quiet_hours(_on: bool) {}
