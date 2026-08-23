//! Minimal file logger.
//!
//! `strpaper` runs as a window-subsystem app with no console, so diagnostics
//! are appended to a log file in the user's `.strland` directory (a sibling of
//! the watched `strpaper` directory, so writes never re-trigger a reload):
//!
//! ```text
//! %USERPROFILE%\.strland\strpaper.log
//! ```

use std::io::Write;

/// Append a line to the strpaper log.
pub fn log(msg: &str) {
    let dir = crate::storage::wallpaper_dir();
    let path = dir.parent().map(|p| p.join("strpaper.log")).unwrap_or(dir);
    if let Ok(mut f) = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
    {
        let _ = writeln!(f, "strpaper: {msg}");
    }
}

/// Record startup diagnostics once.
pub fn start(dir: &str) {
    log("======================================");
    log(&format!("wallpaper directory: {dir}"));
}
