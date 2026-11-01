//! `taskboard`: a local Kanban GUI over every project's tasks.
//!
//! Usage: `taskboard [DIR]` — DIR is the project new tasks go in by default.
//! Without it, that's the git repo the current directory is in, if any (or
//! the current directory, if it already has tasks).

mod platform;
mod watch;

use std::path::PathBuf;

use taskboard_core::{DeviceSettings, GlobalSettings, Store, project_path, project_root};

use platform::Desktop;
use taskboard_ui::{App, Gui, theme};

fn main() -> iced::Result {
    let named = std::env::args_os().nth(1).map(PathBuf::from);
    let start = named
        .clone()
        .unwrap_or_else(|| std::env::current_dir().expect("current directory is accessible"));
    let project = project_path(&project_root(&start));
    let store = match Store::open_default() {
        Ok(store) => store,
        Err(e) => {
            eprintln!("taskboard: couldn't open the task store: {e}");
            std::process::exit(1);
        }
    };
    let platform = Desktop::new(
        store.clone(),
        GlobalSettings::default_path(),
        DeviceSettings::default_path(),
    );
    iced::application(
        move || {
            let board = store.load().map_err(|e| e.to_string());
            let mut app = App::new(
                platform.load_settings_now(),
                platform.load_device_settings_now(),
                board,
            );
            // Started from somewhere that isn't a project (like ~), new
            // tasks get no project until one is picked.
            let is_project = named.is_some()
                || project.join(".git").exists()
                || app.board.project(&project).is_some();
            app.default_project = is_project.then(|| project.clone());
            let mut gui = Gui::new(app, platform.clone());
            let start = gui.start();
            (gui, start)
        },
        Gui::update,
        Gui::view,
    )
    .title(Gui::title)
    .subscription(Gui::subscription)
    .theme(Gui::theme)
    .scale_factor(Gui::scale_factor)
    .font(theme::REGULAR_BYTES)
    .font(theme::BOLD_BYTES)
    .font(theme::MONO_BYTES)
    .default_font(theme::REGULAR)
    .window_size((1600.0, 950.0))
    .antialiasing(true)
    .run()
}
