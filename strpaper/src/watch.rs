//! Hot-reload watcher for the wallpaper directory.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// Watches wallpaper dir and flags changes.
pub struct WallpaperWatcher {
    _watcher: RecommendedWatcher,
    dirty: Arc<AtomicBool>,
}

impl WallpaperWatcher {
    /// Start watching `dir`.
    pub fn new(dir: &Path) -> Result<WallpaperWatcher, String> {
        let dirty = Arc::new(AtomicBool::new(false));
        let d = dirty.clone();
        let mut watcher = notify::recommended_watcher(
            move |res: notify::Result<notify::Event>| {
                if res.is_ok() {
                    d.store(true, Ordering::SeqCst);
                }
            },
        )
        .map_err(|e| format!("create watcher failed: {e}"))?;

        watcher
            .watch(dir, RecursiveMode::NonRecursive)
            .map_err(|e| format!("watch failed: {e}"))?;

        Ok(WallpaperWatcher {
            _watcher: watcher,
            dirty,
        })
    }

    /// Returns true if changed since last check.
    pub fn is_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }
}
