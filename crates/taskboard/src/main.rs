//! `taskboard`: a local Kanban GUI over the project's `.tb/` tasks.
//!
//! Usage: `taskboard [DIR]` — DIR defaults to the current directory; the
//! project is the enclosing git repo's root if there is one.

mod app;
mod markdown;
mod ui;
mod watch;

use std::path::PathBuf;

use ply_engine::prelude::*;
use taskboard_core::{GlobalSettings, Store};

use app::App;
use watch::TaskWatcher;

fn window_conf() -> Conf {
    Conf {
        window_title: "Taskboard".to_string(),
        window_width: 1600,
        window_height: 950,
        window_resizable: true,
        high_dpi: true,
        sample_count: 4,
        ..Default::default()
    }
}

#[macroquad::main(window_conf)]
async fn main() {
    let start = std::env::args_os()
        .nth(1)
        .map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let store = match Store::discover(&start) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("taskboard: couldn't open {}: {e}", start.display());
            std::process::exit(1);
        }
    };

    let watcher = TaskWatcher::new(store.dir(), &store.tasks_path());
    let mut app = App::new(store, GlobalSettings::default_path());
    if let Err(e) = &watcher {
        app.error = Some(format!("Live updates are off; couldn't watch .tb/: {e}"));
    }

    let mut ply = Ply::<()>::new(&ui::FONT).await;
    // Culling tests each element's unrotated layout box, which wrongly drops
    // the rotated titles on collapsed columns near the window edges.
    ply.set_culling(false);
    let mut state = ui::UiState::default();

    loop {
        if watcher.as_ref().is_ok_and(|w| w.changed()) {
            app.reload();
        }

        ui::ensure_fonts().await;
        clear_background(Color::from(ui::BG).into());
        let mut frame = ply.begin();
        let actions = ui::frame(&mut frame, &app, &mut state);
        frame.show(|_| {}).await;
        for action in actions {
            app.apply(action);
        }
        next_frame().await;
    }
}
