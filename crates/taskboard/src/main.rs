//! `taskboard`: a local Kanban GUI over the project's `.tb/` tasks.
//!
//! Usage: `taskboard [DIR]` — DIR defaults to the current directory; the
//! project is the enclosing git repo's root if there is one.

mod app;
mod markdown;
mod source_link;
mod ui;
mod watch;

use std::path::PathBuf;

use taskboard_core::{GlobalSettings, Store};

use app::App;
use ui::Gui;
use ui::theme;

fn main() -> iced::Result {
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
    iced::application(
        move || Gui::new(App::new(store.clone(), GlobalSettings::default_path())),
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
