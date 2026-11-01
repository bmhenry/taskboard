//! Phase 0 spike (task 0DC4FM): the desktop GUI's own source, compiled for
//! `wasm32-unknown-unknown` against an in-memory store seeded from
//! `fixture/tasks.json`. See SPIKE.md. The modules are the desktop crate's
//! files, included by path so nothing moves before Phase 1's crate split.

#[cfg(target_arch = "wasm32")]
#[path = "../../taskboard/src/app.rs"]
mod app;
#[cfg(target_arch = "wasm32")]
mod keyboard;
#[cfg(target_arch = "wasm32")]
#[path = "../../taskboard/src/markdown.rs"]
mod markdown;
#[cfg(target_arch = "wasm32")]
#[path = "../../taskboard/src/source_link.rs"]
mod source_link;
#[cfg(any(target_arch = "wasm32", test))]
mod text_diff;
#[cfg(target_arch = "wasm32")]
#[path = "../../taskboard/src/ui/mod.rs"]
mod ui;
#[cfg(target_arch = "wasm32")]
mod watch;

#[cfg(target_arch = "wasm32")]
fn main() -> iced::Result {
    use std::path::{Path, PathBuf};

    use taskboard_core::Store;

    use app::App;
    use ui::{Gui, theme};

    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    taskboard_core::store::memory::seed(include_str!("../fixture/tasks.json"));

    iced::application(
        || {
            // The in-memory store ignores its directory.
            let store = Store::open(Path::new("/tb")).expect("in-memory store opens");
            let mut app = App::new(store, None);
            app.default_project = Some(PathBuf::from("/home/demo/webshop"));
            Gui::new(app)
        },
        update,
        view,
    )
    .title(Gui::title)
    .subscription(Gui::subscription)
    .theme(Gui::theme)
    .scale_factor(Gui::scale_factor)
    .font(theme::REGULAR_BYTES)
    .font(theme::BOLD_BYTES)
    .font(theme::MONO_BYTES)
    .default_font(theme::REGULAR)
    .antialiasing(true)
    .run()
}

/// `Gui::view` with the soft-keyboard bridge for phones (see `keyboard.rs`).
#[cfg(target_arch = "wasm32")]
fn view(gui: &ui::Gui) -> iced::Element<'_, ui::Message> {
    keyboard::bridge(gui.view())
}

/// `Gui::update`, logging to the browser console when timer and frame
/// subscriptions fire, which is what the spike checks they do on wasm.
#[cfg(target_arch = "wasm32")]
fn update(gui: &mut ui::Gui, message: ui::Message) -> iced::Task<ui::Message> {
    use std::sync::atomic::{AtomicU32, Ordering};

    static FRAMES: AtomicU32 = AtomicU32::new(0);
    match &message {
        ui::Message::CheckProjects => log::info!("spike: time::every fired (CheckProjects)"),
        ui::Message::Frame(now) => {
            let n = FRAMES.fetch_add(1, Ordering::Relaxed) + 1;
            if n == 1 || n.is_multiple_of(10) {
                log::info!("spike: window::frames fired {n} times (last at {now:?})");
            }
        }
        _ => {}
    }
    gui.update(message)
}

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "taskboard-wasm-spike only runs in a browser: see crates/taskboard-wasm-spike/SPIKE.md"
    );
}
