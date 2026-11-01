//! Phase 0 spike (task 0DC4FM): the desktop GUI's own source, compiled for
//! `wasm32-unknown-unknown` against an in-memory store seeded from
//! `fixture/tasks.json`. See SPIKE.md. The UI is the `taskboard-ui` crate,
//! run with the spike's own `Platform` (`platform.rs`).

#[cfg(target_arch = "wasm32")]
mod keyboard;
#[cfg(target_arch = "wasm32")]
mod platform;
#[cfg(any(target_arch = "wasm32", test))]
mod text_diff;

#[cfg(target_arch = "wasm32")]
fn main() -> iced::Result {
    use std::path::{Path, PathBuf};

    use taskboard_core::{GlobalSettings, Store};

    use taskboard_ui::{App, Gui, theme};

    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);

    taskboard_core::store::memory::seed(include_str!("../fixture/tasks.json"));

    iced::application(
        || {
            // The in-memory store ignores its directory.
            let store = Store::open(Path::new("/tb")).expect("in-memory store opens");
            let board = store.load().map_err(|e| e.to_string());
            let mut app = App::new(Ok(GlobalSettings::default()), board);
            app.default_project = Some(PathBuf::from("/home/demo/webshop"));
            Gui::new(app, platform::Spike::new(store))
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
fn view(gui: &taskboard_ui::Gui) -> iced::Element<'_, taskboard_ui::Message> {
    keyboard::bridge(gui.view())
}

/// `Gui::update`, logging to the browser console when timer and frame
/// subscriptions fire, which is what the spike checks they do on wasm.
#[cfg(target_arch = "wasm32")]
fn update(
    gui: &mut taskboard_ui::Gui,
    message: taskboard_ui::Message,
) -> iced::Task<taskboard_ui::Message> {
    use std::sync::atomic::{AtomicU32, Ordering};

    use taskboard_ui::Message;

    static FRAMES: AtomicU32 = AtomicU32::new(0);
    match &message {
        Message::CheckProjects => log::info!("spike: time::every fired (CheckProjects)"),
        Message::Frame(now) => {
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
