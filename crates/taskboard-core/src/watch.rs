//! Watches the store for changes to `tasks.json` from any process (like
//! `tb`), so the desktop app and the web daemon can show them live.

use std::path::Path;

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, RecursiveMode, Watcher};

/// Calls `on_change`, on the watcher's thread, whenever `tasks_file` (which
/// lives in `store_dir`) may have changed, until the watcher returned is
/// dropped.
///
/// It watches the store's directory rather than `tasks.json` itself,
/// because writes replace the file by renaming over it.
pub fn tasks(
    store_dir: &Path,
    tasks_file: &Path,
    on_change: impl Fn() + Send + 'static,
) -> notify::Result<impl Send + 'static> {
    let file = tasks_file.to_path_buf();
    let mut watcher = notify::recommended_watcher(move |res: notify::Result<notify::Event>| {
        if let Ok(event) = res
            && changes(&event, &file)
        {
            on_change();
        }
    })?;
    watcher.watch(store_dir, RecursiveMode::NonRecursive)?;
    Ok(watcher)
}

/// Whether `event` may have changed `file`.
pub fn changes(event: &notify::Event, file: &Path) -> bool {
    changes_file(event.kind)
        && event
            .paths
            .iter()
            .any(|p| p.file_name() == file.file_name())
}

/// Whether `kind` can mean the file's contents changed. Opening and reading
/// it don't, and counting them would reload forever, since reloading opens it.
fn changes_file(kind: EventKind) -> bool {
    match kind {
        EventKind::Access(AccessKind::Close(AccessMode::Write)) => true,
        EventKind::Access(_) => false,
        // Metadata changes (IN_ATTRIB) come from writes like chmod or touch,
        // never from plain reads, so they can't start a loop.
        _ => true,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc;
    use std::time::Duration;

    use notify::event::{CreateKind, DataChange, MetadataKind, ModifyKind, RemoveKind, RenameMode};

    use super::*;

    #[test]
    fn writes_are_changes() {
        for kind in [
            EventKind::Any,
            EventKind::Other,
            EventKind::Create(CreateKind::File),
            EventKind::Modify(ModifyKind::Any),
            EventKind::Modify(ModifyKind::Data(DataChange::Any)),
            EventKind::Modify(ModifyKind::Metadata(MetadataKind::Any)),
            EventKind::Modify(ModifyKind::Name(RenameMode::To)),
            EventKind::Remove(RemoveKind::File),
            EventKind::Access(AccessKind::Close(AccessMode::Write)),
        ] {
            assert!(changes_file(kind), "{kind:?}");
        }
    }

    #[test]
    fn reads_are_not_changes() {
        for kind in [
            EventKind::Access(AccessKind::Any),
            EventKind::Access(AccessKind::Read),
            EventKind::Access(AccessKind::Open(AccessMode::Any)),
            EventKind::Access(AccessKind::Open(AccessMode::Read)),
            EventKind::Access(AccessKind::Close(AccessMode::Read)),
        ] {
            assert!(!changes_file(kind), "{kind:?}");
        }
    }

    /// Runs `act` on `tasks.json` in a watched directory, and says whether
    /// the watcher reported a change to it within a bounded wait.
    fn reports_change(act: impl FnOnce(&Path)) -> bool {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("tasks.json");
        std::fs::write(&file, "{}").unwrap();
        let (tx, rx) = mpsc::channel();
        let _watcher = tasks(dir.path(), &file, move || {
            let _ = tx.send(());
        })
        .unwrap();
        act(&file);
        rx.recv_timeout(Duration::from_secs(2)).is_ok()
    }

    #[test]
    fn real_watcher_reports_writes_but_not_reads() {
        // As the store saves: a temporary file renamed over the original.
        assert!(reports_change(|file| {
            let mut tmp = tempfile::NamedTempFile::new_in(file.parent().unwrap()).unwrap();
            std::io::Write::write_all(&mut tmp, b"[]").unwrap();
            tmp.persist(file).unwrap();
        }));
        assert!(reports_change(|file| std::fs::write(file, "[]").unwrap()));
        assert!(!reports_change(|file| {
            std::fs::read(file).unwrap();
        }));
    }
}
