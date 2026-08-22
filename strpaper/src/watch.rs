//! Hot-reload filesystem watcher for the wallpaper directory.
//!
//! The watcher watches the wallpaper directory non-recursively. Any relevant
//! change flips a shared flag; the main loop picks it up on the next timer
//! tick and reloads the wallpaper.

use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// A watcher over the wallpaper directory that records pending changes.
pub struct WallpaperWatcher {
    /// The OS watcher handle; keeping it alive keeps the watch active.
    _watcher: RecommendedWatcher,
    dirty: Arc<AtomicBool>,
}

impl WallpaperWatcher {
    /// Start watching `dir` (which must already exist).
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

    /// Return `true` if a change was detected since the last call, consuming
    /// the pending flag in the process.
    pub fn is_dirty(&self) -> bool {
        self.dirty.swap(false, Ordering::SeqCst)
    }
}
