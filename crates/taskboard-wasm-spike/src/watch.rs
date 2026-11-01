//! Stand-in for the desktop crate's `watch.rs`, which uses `notify`. There's
//! no filesystem to watch in a browser; Phase 2 replaces this with SSE.

use std::path::Path;

use iced::Subscription;
use taskboard_ui::WatchEvent;

pub fn subscription(_store_dir: &Path, _tasks_file: &Path) -> Subscription<WatchEvent> {
    Subscription::none()
}
