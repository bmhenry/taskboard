//! Taskboard's iced UI, shared by the desktop app (`crates/taskboard`) and
//! the web client. [`App`] holds the board and applies the user's actions to
//! it; [`Gui`] renders it, turns input into those actions, and carries out
//! what they leave for the [`Platform`], like changing the store.
//!
//! The host program implements [`Platform`], loads the settings and board
//! into an [`App`], and runs a [`Gui`] with `iced::application`, loading the
//! [`theme`] fonts.

mod app;
mod markdown;
mod platform;
mod source_link;
#[cfg(test)]
mod testing;
mod ui;

pub use app::App;
pub use platform::{BoxFuture, PathKind, Platform, WatchEvent};
pub use ui::{Gui, Message, theme};
