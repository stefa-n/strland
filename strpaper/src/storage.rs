//! Wallpaper storage and file selection.
//!
//! All wallpaper files are stored in a per-user application directory resolved
//! at runtime from the current Windows account:
//!
//! ```text
//! %USERPROFILE%\.strland\strpaper\
//! ```
//!
//! For example: `C:\Users\<SomeUser>\.strland\strpaper\`
//!
//! The path is never hard-coded and never taken from the directory that
//! contains the `strpaper.exe` executable, so the same binary works for every
//! Windows account. The directory is created automatically if it does not
//! exist.

use std::env;
use std::path::{Path, PathBuf};

/// The fixed filename prefix used for every supported wallpaper.
pub const WALLPAPER_BASENAME: &str = "wallpaper";

/// Optional configuration file inside the wallpaper directory.
///
/// ```toml
/// # Which file in this directory to show, e.g.:
/// wallpaper = "sunset.mp4"
/// ```
///
/// Omitting it keeps the default behaviour: the most recently modified
/// `wallpaper.<ext>` file is shown.
pub const CONFIG_FILE_NAME: &str = "config.toml";

/// The recognised wallpaper file extensions, in canonical order.
///
/// Any `wallpaper.<ext>` file inside the wallpaper directory is a candidate.
pub const SUPPORTED_EXTENSIONS: &[&str] = &["png", "jpg", "jpeg", "bmp", "gif", "mp4", "webm"];

/// The name of the wallpaper directory that lives under the user's `.strland`
/// directory.
pub const WALLPAPER_DIR_NAME: &str = "strpaper";

/// A wallpaper candidate found on disk.
#[derive(Debug, Clone)]
pub struct Candidate {
    pub path: PathBuf,
    pub modified: std::time::SystemTime,
}

/// Resolve the wallpaper directory for the current user.
///
/// Resolution order:
/// 1. `%USERPROFILE%` environment variable (current Windows user's home).
/// 2. `%USERPROFILE%` fall-through to the parent home + `.strland\strpaper`.
///
/// The directory is created (recursively) if it does not already exist.
pub fn wallpaper_dir() -> PathBuf {
    let dir = wallpaper_dir_uncreated();
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Resolve the wallpaper directory but do not create it.
pub fn wallpaper_dir_uncreated() -> PathBuf {
    let home = home_dir();
    home.join(".strland").join(WALLPAPER_DIR_NAME)
}

/// Determine the current user's home directory.
pub fn home_dir() -> PathBuf {
    if let Ok(p) = env::var("USERPROFILE") {
        if !p.is_empty() {
            return PathBuf::from(p);
        }
    }
    let _ = Path::new(".");
    // Last resort: `%HOMEDRIVE%%HOMEPATH%` (e.g. `C:\Users\SomeUser`).
    let drive = env::var("HOMEDRIVE").unwrap_or_else(|_| "C:".into());
    let path = env::var("HOMEPATH").unwrap_or_else(|_| "\\".into());
    PathBuf::from(format!("{drive}{path}"))
}

/// Scan the wallpaper directory for supported `wallpaper.*` files.
///
/// This deliberately only looks for files whose file stem is exactly
/// `wallpaper` and whose extension is one of [`SUPPORTED_EXTENSIONS`]. No
/// arbitrary directories are scanned.
pub fn list_candidates(dir: &Path) -> Vec<Candidate> {
    let mut out = Vec::new();
    let Ok(entries) = std::fs::read_dir(dir) else {
        return out;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let is_file = entry.file_type().map(|t| t.is_file()).unwrap_or(false);
        if !is_file {
            continue;
        }
        let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
            continue;
        };
        let ext = ext.to_ascii_lowercase();
        if !SUPPORTED_EXTENSIONS.contains(&ext.as_str()) {
            continue;
        }
        let stem_matches = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.eq_ignore_ascii_case(WALLPAPER_BASENAME))
            .unwrap_or(false);
        if !stem_matches {
            continue;
        }
        let modified = std::fs::metadata(&path)
            .and_then(|m| m.modified())
            .unwrap_or_else(|_| std::time::SystemTime::UNIX_EPOCH);
        out.push(Candidate { path, modified });
    }
    out
}

/// Choose the active wallpaper from the list of candidates.
///
/// Priority (deterministic, documented in the README):
/// 1. If more than one candidate exists, prefer the file whose last modified
///    time is most recent (most recently replaced).
/// 2. On an exact tie (identical modification time, e.g. from a quick
///    sequence) fall back to the extension order defined by
///    [`SUPPORTED_EXTENSIONS`] and finally to a lexicographic sort so the
///    result is fully deterministic.
pub fn choose_primary(candidates: &mut Vec<Candidate>) -> Option<Candidate> {
    if candidates.is_empty() {
        return None;
    }
    candidates.sort_by(|a, b| {
        b.modified
            .cmp(&a.modified)
            .then_with(|| ext_rank(&a.path).cmp(&ext_rank(&b.path)))
            .then_with(|| a.path.cmp(&b.path))
    });
    Some(candidates.remove(0))
}

/// Path of the optional config file inside the wallpaper directory.
pub fn config_path(dir: &Path) -> PathBuf {
    dir.join(CONFIG_FILE_NAME)
}

/// Read the configured wallpaper file name (the `wallpaper` key), if any.
///
/// Deliberately dependency-free: one `key = "value"` line, `#` comments.
pub fn read_configured_name(dir: &Path) -> Option<String> {
    let text = std::fs::read_to_string(config_path(dir)).ok()?;
    parse_wallpaper_name(&text)
}

/// Read the optional `quality` key — the height videos are pre-scaled to
/// before playback (e.g. `"1080p"`, `"720p"`, `1080`). `None` keeps the
/// source resolution.
pub fn read_configured_quality(dir: &Path) -> Option<u32> {
    let text = std::fs::read_to_string(config_path(dir)).ok()?;
    parse_quality(&text)
}

fn parse_quality(text: &str) -> Option<u32> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("quality") {
            continue;
        }
        let value = value.trim().trim_matches('"').trim().to_ascii_lowercase();
        if value.is_empty() || value == "source" || value == "original" {
            return None;
        }
        let digits = value.trim_end_matches('p');
        if let Ok(h) = digits.parse::<u32>() {
            return Some(h);
        }
    }
    None
}

fn parse_wallpaper_name(text: &str) -> Option<String> {
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        if !key.trim().eq_ignore_ascii_case("wallpaper") {
            continue;
        }
        let value = value.trim();
        // Strip one layer of quotes if present.
        let value = value
            .strip_prefix('"')
            .and_then(|v| v.strip_suffix('"'))
            .map(str::trim)
            .unwrap_or(value);
        return if value.is_empty() { None } else { Some(value.to_string()) };
    }
    None
}

/// Resolve the wallpaper file to display.
///
/// With a valid configured name that exists in the directory, that file is
/// used. Otherwise the default priority rules apply (see [`choose_primary`]).
pub fn resolve_wallpaper_file(dir: &Path, name: Option<&str>) -> Option<PathBuf> {
    if let Some(name) = name {
        let rel = Path::new(name);
        // Only bare file names inside the wallpaper dir are allowed.
        if rel.components().count() == 1 {
            let supported = rel
                .extension()
                .and_then(|e| e.to_str())
                .map(|e| {
                    SUPPORTED_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str())
                })
                .unwrap_or(false);
            if supported {
                let path = dir.join(rel);
                if path.is_file() {
                    return Some(path);
                }
            }
        }
        // Fall through: invalid/missing configuration falls back to defaults.
    }
    let mut candidates = list_candidates(dir);
    choose_primary(&mut candidates).map(|c| c.path)
}

fn ext_rank(path: &Path) -> usize {
    let ext = path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
        .unwrap_or_default();
    SUPPORTED_EXTENSIONS
        .iter()
        .position(|e| *e == ext)
        .unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn touched(path: &Path, unix_secs: u64) {
        let t = filetime::FileTime::from_unix_time(unix_secs as i64, 0);
        filetime::set_file_mtime(path, t).unwrap();
    }

    #[test]
    fn resolves_user_home_dir() {
        let dir = wallpaper_dir_uncreated();
        assert!(dir.ends_with(".strland\\strpaper")); // on Windows
    }

    #[test]
    fn picks_most_recently_modified() {
        let tmp = tempdir();
        let png = tmp.join("wallpaper.png");
        let gif = tmp.join("wallpaper.gif");
        std::fs::write(&png, b"x").unwrap();
        std::fs::write(&gif, b"x").unwrap();
        touched(&png, 1000);
        touched(&gif, 2000); // most recent

        let mut candidates = list_candidates(&tmp);
        let chosen = choose_primary(&mut candidates).unwrap();
        assert_eq!(chosen.path, gif);
    }

    #[test]
    fn falls_back_to_extension_order_on_tie() {
        let tmp = tempdir();
        let png = tmp.join("wallpaper.png");
        let gif = tmp.join("wallpaper.gif");
        std::fs::write(&png, b"x").unwrap();
        std::fs::write(&gif, b"x").unwrap();
        touched(&png, 1000);
        touched(&gif, 1000); // identical mtime

        let mut candidates = list_candidates(&tmp);
        let chosen = choose_primary(&mut candidates).unwrap();
        // png appears before gif in SUPPORTED_EXTENSIONS.
        assert_eq!(chosen.path, png);
    }

    #[test]
    fn empty_dir_yields_none() {
        let tmp = tempdir();
        assert!(choose_primary(&mut list_candidates(&tmp)).is_none());
    }

    fn tempdir() -> PathBuf {
        use std::sync::atomic::{AtomicUsize, Ordering};
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let n = COUNTER.fetch_add(1, Ordering::SeqCst);
        let d = std::env::temp_dir().join(format!(
            "strpaper-test-{}-{}",
            std::process::id(),
            n
        ));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }
}
