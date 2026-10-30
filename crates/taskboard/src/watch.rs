//! Watches `.tb/` so edits from other processes (like `tb`) show up live.

use std::path::{Path, PathBuf};

use iced::Subscription;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream};
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

/// What the watcher reports to the UI.
#[derive(Debug, Clone)]
pub enum WatchEvent {
    /// `tasks.json` changed.
    Changed,
    /// The watcher couldn't start; live updates are off.
    Failed(String),
}

/// Reports changes to `tasks_file`, which lives in `tb_dir`.
pub fn subscription(tb_dir: &Path, tasks_file: &Path) -> Subscription<WatchEvent> {
    Subscription::run_with(
        (tb_dir.to_path_buf(), tasks_file.to_path_buf()),
        |(dir, file)| watch(dir.clone(), file.clone()),
    )
}

/// Watches the `.tb/` directory rather than `tasks.json` itself, because
/// writes replace the file by renaming over it.
fn watch(tb_dir: PathBuf, tasks_file: PathBuf) -> impl Stream<Item = WatchEvent> {
    iced::stream::channel(16, async move |mut out: mpsc::Sender<WatchEvent>| {
        let file_name = tasks_file.file_name().map(|n| n.to_os_string());
        let tx = out.clone();
        let watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
            if let Ok(event) = res
                && event
                    .paths
                    .iter()
                    .any(|p| p.file_name().map(|n| n.to_os_string()) == file_name)
            {
                // A full queue already has a change waiting, so dropping this is fine.
                let _ = tx.clone().try_send(WatchEvent::Changed);
            }
        })
        .and_then(|mut w: RecommendedWatcher| {
            w.watch(&tb_dir, RecursiveMode::NonRecursive)?;
            Ok(w)
        });
        match watcher {
            Ok(_watcher) => {
                // Catch changes made between loading the board and watching it.
                let _ = out.send(WatchEvent::Changed).await;
                // Keep the watcher alive for as long as the subscription runs.
                std::future::pending::<()>().await
            }
            Err(e) => {
                let _ = out.send(WatchEvent::Failed(e.to_string())).await;
            }
        }
    })
}
