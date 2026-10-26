//! Watches `.tb/` so edits from other processes (like `tb`) show up live.

use std::path::Path;
use std::sync::mpsc::{Receiver, channel};

use notify::{RecommendedWatcher, RecursiveMode, Watcher};

pub struct TaskWatcher {
    _watcher: RecommendedWatcher,
    rx: Receiver<()>,
}

impl TaskWatcher {
    /// Watches the `.tb/` directory rather than `tasks.json` itself, because
    /// writes replace the file by renaming over it.
    pub fn new(tb_dir: &Path, tasks_file: &Path) -> notify::Result<TaskWatcher> {
        let (tx, rx) = channel();
        let file_name = tasks_file.file_name().map(|n| n.to_os_string());
        let mut watcher =
            notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
                if let Ok(event) = res
                    && event
                        .paths
                        .iter()
                        .any(|p| p.file_name().map(|n| n.to_os_string()) == file_name)
                {
                    let _ = tx.send(());
                }
            })?;
        watcher.watch(tb_dir, RecursiveMode::NonRecursive)?;
        Ok(TaskWatcher {
            _watcher: watcher,
            rx,
        })
    }

    /// True if `tasks.json` changed since the last call.
    pub fn changed(&self) -> bool {
        let mut any = false;
        while self.rx.try_recv().is_ok() {
            any = true;
        }
        any
    }
}
