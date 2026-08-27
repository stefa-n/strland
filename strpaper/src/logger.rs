//! Minimal file logger — appends to `%USERPROFILE%\.strland\strpaper.log`.

/// Append to log file (debug only, no-op in release).
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
