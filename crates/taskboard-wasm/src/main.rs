//! `taskboard-wasm`: Taskboard's web client. It's the `taskboard-ui` crate's
//! UI, built for the browser by trunk and served by the `taskboard-web`
//! daemon, run with the web [`platform::Web`] over the daemon's API. See
//! README.md.

#[cfg(target_arch = "wasm32")]
mod api;
#[cfg(target_arch = "wasm32")]
mod keyboard;
#[cfg(target_arch = "wasm32")]
mod platform;
#[cfg(any(target_arch = "wasm32", test))]
mod text_diff;

#[cfg(target_arch = "wasm32")]
fn main() {
    console_error_panic_hook::set_once();
    let _ = console_log::init_with_level(log::Level::Info);
    // iced only starts winit's event loop and returns, so the page stays
    // responsive while the board loads.
    wasm_bindgen_futures::spawn_local(async {
        if let Err(e) = run().await {
            log::error!("taskboard: {e}");
        }
    });
}

/// Loads the settings and board from the daemon, then starts the UI, which
/// shows why either couldn't be loaded.
#[cfg(target_arch = "wasm32")]
async fn run() -> iced::Result {
    use taskboard_ui::{App, Gui, theme};

    let (settings, board) = iced::futures::join!(platform::load_settings(), platform::load_board());
    let location = settings.as_ref().ok().and_then(|r| r.location.clone());
    let settings = settings.map(|r| r.settings);
    let default_project = board.as_ref().ok().and_then(|r| r.default_project.clone());
    let board = board.map(|r| r.board);
    let device = platform::load_device_settings_now();
    iced::application(
        move || {
            let mut app = App::new(settings.clone(), device.clone(), board.clone());
            app.default_project = default_project.clone();
            let mut gui = Gui::new(app, platform::Web::new(location.clone()));
            let start = gui.start();
            (gui, start)
        },
        Gui::update,
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

#[cfg(not(target_arch = "wasm32"))]
fn main() {
    eprintln!(
        "taskboard-wasm only runs in a browser; build it with trunk: see crates/taskboard-wasm/README.md"
    );
}
