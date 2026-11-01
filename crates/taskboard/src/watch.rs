//! Watches the task store so edits from other processes (like `tb`) show up live.

use std::path::{Path, PathBuf};

use iced::Subscription;
use iced::futures::channel::mpsc;
use iced::futures::{SinkExt, Stream};
use taskboard_ui::WatchEvent;

/// Reports changes to `tasks_file`, which lives in `store_dir`.
pub fn subscription(store_dir: &Path, tasks_file: &Path) -> Subscription<WatchEvent> {
    Subscription::run_with(
        (store_dir.to_path_buf(), tasks_file.to_path_buf()),
        |(dir, file)| watch(dir.clone(), file.clone()),
    )
}

fn watch(store_dir: PathBuf, tasks_file: PathBuf) -> impl Stream<Item = WatchEvent> {
    iced::stream::channel(16, async move |mut out: mpsc::Sender<WatchEvent>| {
        let tx = out.clone();
        let watcher = taskboard_core::watch::tasks(&store_dir, &tasks_file, move || {
            // A full queue already has a change waiting, so dropping this is fine.
            let _ = tx.clone().try_send(WatchEvent::Changed);
        });
        match watcher {
            Ok(_watcher) => {
                // Catch changes made between loading the board and watching it.
                let _ = out.send(WatchEvent::Changed).await;
                // Keep the watcher alive for as long as the subscription runs.
                std::future::pending::<()>().await
            }
            Err(e) => {
                let why = format!("couldn't watch {}: {e}", store_dir.display());
                let _ = out.send(WatchEvent::Failed(why)).await;
            }
        }
    })
}
