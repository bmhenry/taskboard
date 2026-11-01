//! Taskboard's iced UI, shared by the desktop app (`crates/taskboard`) and
//! the web client. [`App`] holds the board and applies the user's actions to
//! the store; [`Gui`] renders it and turns input into those actions.
//!
//! The host program runs a [`Gui`] with `iced::application`, loads the
//! [`theme`] fonts, and passes [`Gui::watching`] a [`BoardWatcher`] for live
//! updates.

mod app;
mod markdown;
mod source_link;
mod ui;

pub use app::App;
pub use ui::{BoardWatcher, Gui, Message, WatchEvent, theme};
