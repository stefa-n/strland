//! Minimal file logger.
//!
//! `strpaper` runs as a window-subsystem app with no console, so diagnostics
//! are appended to a log file in the user's `.strland` directory (a sibling of
//! the watched `strpaper` directory, so writes never re-trigger a reload):
//!
//! ```text
//! %USERPROFILE%\.strland\strpaper.log
//! ```

/// Append a line to the strpaper log.
///
/// Diagnostics are a development aid only: release builds compile this to
/// nothing, so no log file is ever written.
pub fn log(msg: &str) {
    #[cfg(debug_assertions)]
    {
        use std::io::Write;
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
    #[cfg(not(debug_assertions))]
    {
        let _ = msg;
    }
}
