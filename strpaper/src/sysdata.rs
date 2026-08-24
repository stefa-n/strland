//! System data sources for widgets: audio spectrum, media info, battery.
//!
//! Audio: WASAPI loopback capture + FFT → 32-band spectrum (same approach as
//! strbar's pill visualizer, but with more bands for the bigger widget).
//! Media: WinRT SMTC (GlobalSystemMediaTransportControlsSessionManager).
//! Battery: GetSystemPowerStatus for internal + SetupAPI for BT devices.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

// ---------------------------------------------------------------------------
// Audio spectrum (WASAPI loopback + FFT)
// ---------------------------------------------------------------------------

pub const AUDIO_BANDS: usize = 32;

static AUDIO_BAND_LEVELS: [AtomicU32; AUDIO_BANDS] = {
    #[allow(clippy::declare_interior_mutable_const)]
    const ZERO: AtomicU32 = AtomicU32::new(0);
    [
        ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO,
        ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO, ZERO,
        ZERO, ZERO,
    ]
};

pub fn audio_bands() -> [f32; AUDIO_BANDS] {
    let mut out = [0.0f32; AUDIO_BANDS];
    for (i, band) in out.iter_mut().enumerate() {
        *band = AUDIO_BAND_LEVELS[i].load(Ordering::Relaxed) as f32 / 4096.0;
    }
    out
}

static AUDIO_RUNNING: AtomicBool = AtomicBool::new(false);

pub fn start_audio_spectrum_poller() {
    if AUDIO_RUNNING.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("widget-audio".into())
        .spawn(|| {
            use windows::Win32::System::Com::{CoInitializeEx, COINIT_MULTITHREADED};
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

fn store_bands(bands: &[f32; AUDIO_BANDS]) {
    for (i, v) in bands.iter().enumerate() {
        let scaled = (v.clamp(0.0, 1.0) * 4096.0).round() as u32;
        AUDIO_BAND_LEVELS[i].store(scaled, Ordering::Relaxed);
    }
}

const FFT_SIZE: usize = 1024;

unsafe fn run_loopback_analyzer(smooth: &mut [f32; AUDIO_BANDS]) -> windows::core::Result<()> { unsafe {
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::*;

    let enumerator: IMMDeviceEnumerator =
        CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)?;
    let device = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
    let initial_id = {
        let id = device.GetId()?;
        let s = id.to_string()?;
        CoTaskMemFree(Some(id.as_ptr().cast()));
        s
    };
    let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
    let mix_format = client.GetMixFormat()?;
    let channels = ((*mix_format).nChannels as usize).max(1);
    let format_tag = (*mix_format).wFormatTag;
    let cb_size = (*mix_format).cbSize;
    let is_float = if format_tag == 3 {
        true
    } else if format_tag == 0xFFFE && cb_size as usize >= 22 {
        const IEEE_FLOAT: windows::core::GUID =
            windows::core::GUID::from_u128(0x0000_0003_0000_0010_8000_00AA_0038_9B71);
        let ext = mix_format as *const WAVEFORMATEXTENSIBLE;
        std::ptr::addr_of!((*ext).SubFormat).read_unaligned() == IEEE_FLOAT
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
        if ticks % 200 == 0 {
            let current = enumerator.GetDefaultAudioEndpoint(eRender, eConsole)?;
            let id = current.GetId()?;
            let id_str = id.to_string()?;
            CoTaskMemFree(Some(id.as_ptr().cast()));
            if id_str != initial_id {
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
                let bpf = channels * if is_float { 4 } else { 2 };
                let slice = std::slice::from_raw_parts(data, frames as usize * bpf);
                for frame in 0..frames as usize {
                    let base = frame * bpf;
                    let mut mono = 0.0f32;
                    for ch in 0..channels {
                        let s = if is_float {
                            f32::from_le_bytes([
                                slice[base + ch * 4],
                                slice[base + ch * 4 + 1],
                                slice[base + ch * 4 + 2],
                                slice[base + ch * 4 + 3],
                            ])
                        } else {
                            i16::from_le_bytes([slice[base + ch * 2], slice[base + ch * 2 + 1]])
                                as f32
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
            let mut ordered = vec![0.0f32; FFT_SIZE];
            ordered[..FFT_SIZE - write_pos].copy_from_slice(&window[write_pos..]);
            ordered[FFT_SIZE - write_pos..].copy_from_slice(&window[..write_pos]);
            let mut target = [0.0f32; AUDIO_BANDS];
            compute_spectrum(&ordered, &mut target);
            for i in 0..AUDIO_BANDS {
                let factor = if target[i] > smooth[i] { 0.55 } else { 0.18 };
                smooth[i] += (target[i] - smooth[i]) * factor;
            }
            store_bands(smooth);
        }
    }
}}

fn compute_spectrum(samples: &[f32], bands: &mut [f32; AUDIO_BANDS]) {
    let n = samples.len();
    if !n.is_power_of_two() || n < 64 {
        return;
    }
    let mut re: Vec<f32> = samples
        .iter()
        .enumerate()
        .map(|(i, &s)| s * (0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / n as f32).cos()))
        .collect();
    let mut im = vec![0.0f32; n];

    // Iterative radix-2 Cooley-Tukey FFT
    let mut j = 0usize;
    for i in 0..n {
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
        let mut k = n >> 1;
        while k > 0 && j & k != 0 {
            j ^= k;
            k >>= 1;
        }
        j |= k;
    }
    let mut len = 2;
    while len <= n {
        let half = len / 2;
        let angle = -2.0 * std::f32::consts::PI / len as f32;
        let wr = angle.cos();
        let wi = angle.sin();
        for start in (0..n).step_by(len) {
            let mut tw_r = 1.0f32;
            let mut tw_i = 0.0f32;
            for k in start..start + half {
                let ur = re[k];
                let ui = im[k];
                let vr = re[k + half] * tw_r - im[k + half] * tw_i;
                let vi = re[k + half] * tw_i + im[k + half] * tw_r;
                re[k] = ur + vr;
                im[k] = ui + vi;
                re[k + half] = ur - vr;
                im[k + half] = ui - vi;
                let new_wr = tw_r * wr - tw_i * wi;
                tw_i = tw_r * wi + tw_i * wr;
                tw_r = new_wr;
            }
        }
        len <<= 1;
    }

    // Map bins to bands (logarithmic grouping for better visual distribution)
    let band_count = bands.len();
    for (band, level) in bands.iter_mut().enumerate() {
        let lo = (band * n / 2 / band_count).max(1);
        let hi = ((band + 1) * n / 2 / band_count).max(lo + 1);
        let sum: f32 = (lo..hi.min(n / 2))
            .map(|k| (re[k] * re[k] + im[k] * im[k]).sqrt())
            .sum();
        let count = (hi - lo).max(1) as f32;
        *level = (sum / count / n as f32 * 4.0).clamp(0.0, 1.0);
    }
}

// ---------------------------------------------------------------------------
// Media info (WinRT SMTC)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug, Default)]
pub struct MediaInfo {
    #[allow(dead_code)]
    pub title: String,
    #[allow(dead_code)]
    pub artist: String,
    pub playing: bool,
}

static MEDIA_INFO: OnceLock<Mutex<MediaInfo>> = OnceLock::new();
static MEDIA_POLL_STARTED: AtomicBool = AtomicBool::new(false);

pub fn start_media_poller() {
    if MEDIA_POLL_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("widget-media".into())
        .spawn(|| {
            loop {
                let info = poll_media_info();
                if let Ok(mut m) = MEDIA_INFO.get_or_init(|| Mutex::new(MediaInfo::default())).lock() {
                    *m = info;
                }
                std::thread::sleep(std::time::Duration::from_millis(500));
            }
        })
        .ok();
}

pub fn media_info() -> MediaInfo {
    MEDIA_INFO
        .get_or_init(|| Mutex::new(MediaInfo::default()))
        .lock()
        .ok()
        .map(|m| m.clone())
        .unwrap_or_default()
}

#[cfg(target_os = "windows")]
fn poll_media_info() -> MediaInfo {
    use windows::Media::Control::GlobalSystemMediaTransportControlsSessionManager;
    let manager = GlobalSystemMediaTransportControlsSessionManager::RequestAsync()
        .and_then(|op| op.get())
        .ok();
    let Some(manager) = manager else {
        return MediaInfo::default();
    };
    let sessions = manager.GetSessions().ok();
    let session = sessions
        .and_then(|s| s.First().ok())
        .and_then(|it| it.Current().ok());
    let Some(session) = session else {
        return MediaInfo::default();
    };
    let playback = session.GetPlaybackInfo().ok();
    let status = playback
        .and_then(|p| p.PlaybackStatus().ok())
        .map(|s| s.0)
        .unwrap_or(0);
    let playing = status == 4; // PLAYING
    let props = session.TryGetMediaPropertiesAsync().and_then(|op| op.get()).ok();
    let Some(props) = props else {
        return MediaInfo { title: String::new(), artist: String::new(), playing };
    };
    MediaInfo {
        title: props.Title().map(|s| s.to_string()).unwrap_or_default(),
        artist: props.Artist().map(|s| s.to_string()).unwrap_or_default(),
        playing,
    }
}

#[cfg(not(target_os = "windows"))]
fn poll_media_info() -> MediaInfo {
    MediaInfo::default()
}

// ---------------------------------------------------------------------------
// Battery
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BatteryInfo {
    pub percent: i64,
    pub charging: bool,
}

static BATTERY: OnceLock<Mutex<(Instant, BatteryInfo)>> = OnceLock::new();

pub fn battery() -> BatteryInfo {
    let mut cache = BATTERY
        .get_or_init(|| {
            Mutex::new((
                Instant::now() - Duration::from_secs(10),
                BatteryInfo { percent: 0, charging: false },
            ))
        })
        .lock()
        .unwrap();
    if cache.0.elapsed() < Duration::from_secs(5) {
        return cache.1.clone();
    }
    let info = query_battery();
    *cache = (Instant::now(), info.clone());
    info
}

fn query_battery() -> BatteryInfo {
    unsafe {
        use windows::Win32::System::Power::{GetSystemPowerStatus, SYSTEM_POWER_STATUS};
        let mut sps = SYSTEM_POWER_STATUS::default();
        if GetSystemPowerStatus(&mut sps).is_err() {
            return BatteryInfo { percent: 0, charging: false };
        }
        BatteryInfo {
            percent: if sps.BatteryLifePercent != 255 {
                sps.BatteryLifePercent as i64
            } else {
                0
            },
            charging: sps.ACLineStatus == 1,
        }
    }
}

// ---------------------------------------------------------------------------
// Audio level accessors
// ---------------------------------------------------------------------------

/// Get the audio level (0-100) at a normalised position (0.0-1.0).
#[allow(dead_code)]
pub fn audio_level_at(pos: f64) -> i64 {
    let bands = audio_bands();
    let idx = (pos.clamp(0.0, 1.0) * (bands.len() - 1) as f64).round() as usize;
    (bands[idx].clamp(0.0, 1.0) * 100.0) as i64
}

/// True when audio is currently playing (peak above silence threshold).
#[allow(dead_code)]
pub fn audio_playing() -> bool {
    let bands = audio_bands();
    let total: f32 = bands.iter().sum();
    (total / bands.len() as f32) > 0.01
}

// ---------------------------------------------------------------------------
// Bluetooth device batteries (SetupAPI battery class enumeration)
// ---------------------------------------------------------------------------

#[derive(Clone, Debug)]
pub struct BtDeviceInfo {
    pub name: String,
    pub level: i64,
}

static BT_CACHE: OnceLock<Mutex<(Instant, Vec<BtDeviceInfo>)>> = OnceLock::new();

pub fn bt_devices() -> Vec<BtDeviceInfo> {
    let mut cache = BT_CACHE
        .get_or_init(|| Mutex::new((Instant::now() - Duration::from_secs(60), Vec::new())))
        .lock()
        .unwrap();
    if cache.0.elapsed() < Duration::from_secs(30) {
        return cache.1.clone();
    }
    let devices = query_bt_batteries();
    *cache = (Instant::now(), devices.clone());
    devices
}

fn query_bt_batteries() -> Vec<BtDeviceInfo> {
    unsafe {
        use windows::Win32::Devices::DeviceAndDriverInstallation::*;
        use windows::core::GUID;

        let mut results = Vec::new();
        let guid = GUID::from_u128(0x72631e54_78a4_11d0_bcf7_00aa00b7b32a);
        let hdev = match SetupDiGetClassDevsW(
            Some(&guid),
            None,
            None,
            DIGCF_PRESENT,
        ) {
            Ok(h) => h,
            Err(_) => return results,
        };

        let mut i = 0u32;
        loop {
            let mut devinfo = SP_DEVINFO_DATA::default();
            devinfo.cbSize = std::mem::size_of::<SP_DEVINFO_DATA>() as u32;
            if SetupDiEnumDeviceInfo(hdev, i, &mut devinfo).is_err() {
                break;
            }

            let mut buf = [0u8; 1024];
            let mut reg_type = 0u32;
            if SetupDiGetDeviceRegistryPropertyW(
                hdev,
                &devinfo,
                SPDRP_FRIENDLYNAME,
                Some(&mut reg_type),
                Some(&mut buf),
                None,
            ).is_ok() {
                let wchars: Vec<u16> = buf
                    .chunks_exact(2)
                    .take_while(|c| *c != [0, 0])
                    .map(|c| u16::from_le_bytes([c[0], c[1]]))
                    .collect();
                let name = String::from_utf16_lossy(&wchars);
                let level = parse_battery_from_name(&name);
                results.push(BtDeviceInfo { name, level });
            }
            i += 1;
        }

        let _ = SetupDiDestroyDeviceInfoList(hdev);
        results
    }
}

fn parse_battery_from_name(name: &str) -> i64 {
    let bytes = name.as_bytes();
    for i in (0..bytes.len()).rev() {
        if bytes[i] == b'%' && i > 0 {
            let start = bytes[..i]
                .iter()
                .rposition(|&b| !b.is_ascii_digit())
                .map(|p| p + 1)
                .unwrap_or(0);
            if let Ok(v) = name[start..i].parse::<i64>() {
                if (0..=100).contains(&v) {
                    return v;
                }
            }
        }
    }
    -1
}

// ---------------------------------------------------------------------------
// GPU utilization (PDH "GPU Engine" performance counters)
// ---------------------------------------------------------------------------

/// Latest total 3D-engine GPU utilisation in percent (0-100).
static GPU_PERCENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
static GPU_POLL_STARTED: AtomicBool = AtomicBool::new(false);

/// Start the background poller that samples GPU utilisation once per second.
pub fn start_gpu_poller() {
    if GPU_POLL_STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    std::thread::Builder::new()
        .name("strpaper-gpu".into())
        .spawn(|| unsafe { gpu_poll_loop() })
        .ok();
}

/// Current GPU utilisation percentage (0-100), sampled by the background
/// poller. Returns 0 before the first sample is available.
pub fn gpu_usage() -> i64 {
    GPU_PERCENT.load(Ordering::Relaxed).min(100) as i64
}

unsafe fn gpu_poll_loop() { unsafe {
    use windows::core::w;
    use windows::Win32::System::Performance::{
        PdhAddEnglishCounterW, PdhCloseQuery, PdhCollectQueryData,
        PdhGetFormattedCounterArrayW, PdhOpenQueryW, PDH_FMT,
        PDH_FMT_COUNTERVALUE_ITEM_W,
    };

    // \GPU Engine(*)\Utilization Percentage formatted as double.
    const PDH_FMT_DOUBLE_FLAG: PDH_FMT = PDH_FMT(0x0000_0200);

    let mut query = 0isize;
    // PDH functions return WIN32_ERROR; 0 == ERROR_SUCCESS.
    if PdhOpenQueryW(None, 0, &mut query) != 0 {
        return;
    }

    // Wildcard counter: one instance per process/engine. We filter to the 3D
    // engine type and sum across all processes for a total-utilisation figure.
    let mut counter = 0isize;
    if PdhAddEnglishCounterW(
        query,
        w!(r"\GPU Engine(*)\Utilization Percentage"),
        0,
        &mut counter,
    ) != 0
    {
        let _ = PdhCloseQuery(query);
        return;
    }

    loop {
        // Rate counters need two samples; with a persistent query every
        // collection after the first yields valid deltas.
        if PdhCollectQueryData(query) != 0 {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // Query required buffer size.
        let mut size = 0u32;
        let mut item_count = 0u32;
        let _ = PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE_FLAG,
            &mut size,
            &mut item_count,
            None,
        );
        if size == 0 {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }
        let mut buf = vec![0u8; size as usize];

        item_count = 0;
        if PdhGetFormattedCounterArrayW(
            counter,
            PDH_FMT_DOUBLE_FLAG,
            &mut size,
            &mut item_count,
            Some(buf.as_mut_ptr().cast()),
        ) != 0
        {
            std::thread::sleep(Duration::from_secs(1));
            continue;
        }

        // Walk the item array: each entry is { szName: PWSTR, value }.
        let item_size = std::mem::size_of::<PDH_FMT_COUNTERVALUE_ITEM_W>();
        if item_size == 0 {
            break;
        }
        let mut total = 0f64;
        for i in 0..item_count as usize {
            let item = buf.as_ptr().add(i * item_size) as *const PDH_FMT_COUNTERVALUE_ITEM_W;
            let item = &*item;
            let name = item.szName.to_string().unwrap_or_default();
            if !name.contains("engtype_3D") {
                continue;
            }
            // PDH_FMT_COUNTERVALUE carries the formatted value in a union;
            // for PDH_FMT_DOUBLE requests the f64 member is the payload and
            // sits right after the 4-byte status word (aligned to 8).
            let val = *(std::ptr::addr_of!(item.FmtValue).cast::<u8>()
                .add(8)
                .cast::<f64>());
            if val.is_finite() && val > 0.0 {
                total += val;
            }
        }
        GPU_PERCENT.store(total.round().clamp(0.0, 100.0) as u32, Ordering::Relaxed);

        std::thread::sleep(Duration::from_secs(1));
    }

    let _ = PdhCloseQuery(query);
}}
