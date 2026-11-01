//! The web [`Platform`]: the store is reached through the daemon's API,
//! changes arrive as server-sent events, files are uploaded from the
//! browser's file picker, and device settings live in the browser's
//! `localStorage`. Folders can't be picked, only typed, and a typed path
//! can't be attached, since it would be read on the daemon's machine.

use std::cell::RefCell;
use std::future::Future;
use std::path::PathBuf;
use std::rc::Rc;

use iced::Subscription;
use iced::futures::channel::{mpsc, oneshot};
use iced::futures::{SinkExt, Stream, StreamExt};
use js_sys::Uint8Array;
use taskboard_core::api::{self, BoardEvent, BoardResponse, PathKind, SettingsResponse};
use taskboard_core::ops::{Command, FileSource, Outcome};
use taskboard_core::{Attachment, Board, DeviceSettings, GlobalSettings, Ulid};
use taskboard_ui::{BoxFuture, Platform, WatchEvent};
use wasm_bindgen::JsCast;
use wasm_bindgen::closure::Closure;
use wasm_bindgen_futures::JsFuture;
use web_sys::{Event, EventSource, HtmlInputElement, MessageEvent};

use crate::api::{Body, UNAUTHORIZED, encode, js_error};

/// The `localStorage` key device settings are kept under.
const DEVICE_KEY: &str = "taskboard.device";

pub struct Web {
    /// Where the daemon saves settings, as it said when they were loaded.
    settings_location: Option<String>,
}

impl Web {
    pub fn new(settings_location: Option<String>) -> Web {
        Web { settings_location }
    }
}

/// The board and what came with it, for starting up.
pub async fn load_board() -> Result<BoardResponse, String> {
    crate::api::json("GET", api::BOARD, Body::None).await
}

/// Settings and where they're saved, for starting up.
pub async fn load_settings() -> Result<SettingsResponse, String> {
    crate::api::json("GET", api::SETTINGS, Body::None).await
}

/// Reads device settings, as [`Platform::load_device_settings`] does but
/// without waiting, for starting up.
pub fn load_device_settings_now() -> Result<DeviceSettings, String> {
    let Some(text) = storage()?.get_item(DEVICE_KEY).map_err(js_error)? else {
        return Ok(DeviceSettings::default());
    };
    serde_json::from_str::<DeviceSettings>(&text)
        .map(DeviceSettings::clamped)
        .map_err(|e| format!("invalid device settings in this browser: {e}"))
}

fn storage() -> Result<web_sys::Storage, String> {
    let window = web_sys::window().ok_or("no browser window")?;
    window
        .local_storage()
        .map_err(js_error)?
        .ok_or_else(|| "this browser has no local storage".to_string())
}

impl Platform for Web {
    fn run(&self, command: Command) -> BoxFuture<Result<Outcome, String>> {
        Box::pin(async move {
            match command {
                // Sent raw rather than as base64 in a command.
                Command::Attach {
                    task,
                    source: FileSource::Bytes { name, bytes },
                } => {
                    let url = format!("{}?task={task}&name={}", api::UPLOADS, encode(&name));
                    crate::api::json("POST", &url, Body::Bytes(bytes)).await
                }
                command => crate::api::json("POST", api::COMMANDS, Body::json(&command)?).await,
            }
        })
    }

    fn load_board(&self) -> BoxFuture<Result<Board, String>> {
        Box::pin(async { load_board().await.map(|r| r.board) })
    }

    fn read_file(&self, task: Ulid, file: &Attachment) -> BoxFuture<Result<Vec<u8>, String>> {
        let url = format!("{}/{task}/{}", api::FILES, file.id);
        Box::pin(async move { crate::api::bytes(&url).await })
    }

    fn board_changes(&self) -> Subscription<WatchEvent> {
        Subscription::run(board_events)
    }

    fn pick_file(&self) -> BoxFuture<Option<FileSource>> {
        Box::pin(pick_file())
    }

    fn pick_folder(&self, _title: &str) -> BoxFuture<Option<PathBuf>> {
        Box::pin(async { None })
    }

    fn picks_folders(&self) -> bool {
        false
    }

    fn attaches_paths(&self) -> bool {
        false
    }

    fn open_url(&self, url: &str) -> Result<(), String> {
        let window = web_sys::window().ok_or("no browser window")?;
        // `noopener`: the page opened can't reach back into this one.
        window
            .open_with_url_and_target_and_features(url, "_blank", "noopener")
            .map(|_| ())
            .map_err(|e| format!("couldn't open {url}: {}", js_error(e)))
    }

    /// Paths the daemon can't be asked about count as folders: when it
    /// can't be reached, every project would otherwise show as missing,
    /// offering to relocate or delete it.
    fn path_kinds(&self, paths: Vec<PathBuf>) -> BoxFuture<Vec<PathKind>> {
        Box::pin(async move {
            let asked = match Body::json(&paths) {
                Ok(body) => crate::api::json("POST", api::PATH_KINDS, body).await,
                Err(e) => Err(e),
            };
            match asked {
                Ok(kinds) => kinds,
                Err(e) => {
                    log::warn!("couldn't check paths: {e}");
                    vec![PathKind::Dir; paths.len()]
                }
            }
        })
    }

    fn load_settings(&self) -> BoxFuture<Result<GlobalSettings, String>> {
        Box::pin(async { load_settings().await.map(|r| r.settings) })
    }

    fn save_settings(&self, settings: GlobalSettings) -> BoxFuture<Result<(), String>> {
        Box::pin(async move {
            crate::api::send("PUT", api::SETTINGS, Body::json(&settings)?)
                .await
                .map(|_| ())
        })
    }

    fn settings_location(&self) -> Option<String> {
        self.settings_location.clone()
    }

    fn load_device_settings(&self) -> BoxFuture<Result<DeviceSettings, String>> {
        let loaded = load_device_settings_now();
        Box::pin(async move { loaded })
    }

    fn save_device_settings(&self, settings: DeviceSettings) -> BoxFuture<Result<(), String>> {
        let saved = serde_json::to_string(&settings)
            .map_err(|e| e.to_string())
            .and_then(|text| storage()?.set_item(DEVICE_KEY, &text).map_err(js_error));
        Box::pin(async move { saved })
    }

    fn device_settings_location(&self) -> Option<String> {
        Some("this browser's local storage".into())
    }
}

/// The daemon's [`api::EVENTS`], as the UI wants them. The browser
/// reconnects by itself after the connection drops, and the daemon then
/// says the board changed, so nothing is missed; only a refusal ends it.
fn board_events() -> impl Stream<Item = WatchEvent> {
    iced::stream::channel(8, async |mut output: mpsc::Sender<WatchEvent>| {
        let (tx, mut rx) = mpsc::unbounded();
        let listener = match Listener::open(tx) {
            Ok(listener) => listener,
            Err(e) => {
                let _ = output.send(WatchEvent::Failed(e)).await;
                return;
            }
        };
        while let Some(event) = rx.next().await {
            let event = match event {
                Some(event) => event,
                // An `EventSource` doesn't say why it was refused, so
                // the API is asked, to tell a sign-in problem apart.
                None => {
                    let why = match load_board().await {
                        Err(e) if e == UNAUTHORIZED => e,
                        _ => {
                            "the daemon closed the connection; reload the page to reconnect".into()
                        }
                    };
                    let _ = output.send(WatchEvent::Failed(why)).await;
                    break;
                }
            };
            if output.send(event).await.is_err() {
                break;
            }
        }
        drop(listener);
    })
}

/// An open `EventSource` on [`api::EVENTS`], sending each event it gets, or
/// `None` once the browser has given up on it. It's closed when dropped.
struct Listener {
    source: EventSource,
    _on_message: Closure<dyn FnMut(MessageEvent)>,
    _on_error: Closure<dyn FnMut(Event)>,
}

impl Listener {
    fn open(tx: mpsc::UnboundedSender<Option<WatchEvent>>) -> Result<Listener, String> {
        let source = EventSource::new(api::EVENTS).map_err(js_error)?;
        let on_message = Closure::<dyn FnMut(MessageEvent)>::new({
            let tx = tx.clone();
            move |e: MessageEvent| {
                let data = e.data().as_string().unwrap_or_default();
                let event = match serde_json::from_str(&data) {
                    Ok(BoardEvent::Changed) => WatchEvent::Changed,
                    Ok(BoardEvent::Failed { reason }) => WatchEvent::Failed(reason),
                    // Reloading is always safe.
                    Err(e) => {
                        log::warn!("unexpected event from the daemon ({e}): {data}");
                        WatchEvent::Changed
                    }
                };
                let _ = tx.unbounded_send(Some(event));
            }
        });
        // Called while reconnecting, too; the browser only stops trying,
        // leaving the source closed, when the daemon refuses it.
        let on_error = Closure::<dyn FnMut(Event)>::new(move |e: Event| {
            let closed = e
                .target()
                .and_then(|t| t.dyn_into::<EventSource>().ok())
                .is_some_and(|s| s.ready_state() == EventSource::CLOSED);
            if closed {
                let _ = tx.unbounded_send(None);
            }
        });
        source.set_onmessage(Some(on_message.as_ref().unchecked_ref()));
        source.set_onerror(Some(on_error.as_ref().unchecked_ref()));
        Ok(Listener {
            source,
            _on_message: on_message,
            _on_error: on_error,
        })
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        self.source.set_onmessage(None);
        self.source.set_onerror(None);
        self.source.close();
    }
}

/// Opens the browser's file picker and reads the file chosen. The picker is
/// opened now, not when the future is first polled, so that it's inside the
/// click that asked for it, as browsers require. Cancelling gives `None`,
/// in browsers that say so (with a `cancel` event); in others the future
/// never finishes, which is harmless.
fn pick_file() -> impl Future<Output = Option<FileSource>> {
    let (tx, rx) = oneshot::channel::<Option<web_sys::File>>();
    let opened = open_picker(tx);
    async move {
        let (input, listeners) = match opened {
            Ok(opened) => opened,
            Err(e) => {
                log::error!("couldn't open the file picker: {e}");
                return None;
            }
        };
        let file = rx.await.ok().flatten();
        input.remove();
        drop(listeners);
        let file = file?;
        match JsFuture::from(file.array_buffer()).await {
            Ok(buffer) => Some(FileSource::Bytes {
                name: file.name(),
                bytes: Uint8Array::new(&buffer).to_vec(),
            }),
            Err(e) => {
                log::error!("couldn't read {}: {}", file.name(), js_error(e));
                None
            }
        }
    }
}

type PickerListeners = [Closure<dyn FnMut(Event)>; 2];

/// Adds a hidden file input to the page and clicks it; `tx` gets the file
/// picked, or `None` if the picker was cancelled.
fn open_picker(
    tx: oneshot::Sender<Option<web_sys::File>>,
) -> Result<(HtmlInputElement, PickerListeners), String> {
    let document = web_sys::window()
        .and_then(|w| w.document())
        .ok_or("no document")?;
    let input: HtmlInputElement = document
        .create_element("input")
        .map_err(js_error)?
        .unchecked_into();
    input.set_type("file");
    input
        .style()
        .set_property("display", "none")
        .map_err(js_error)?;
    // Some mobile browsers only open the picker for an input on the page.
    document
        .body()
        .ok_or("no document body")?
        .append_child(&input)
        .map_err(js_error)?;
    let tx = Rc::new(RefCell::new(Some(tx)));
    let send = move |file: Option<web_sys::File>| {
        if let Some(tx) = tx.borrow_mut().take() {
            let _ = tx.send(file);
        }
    };
    let on_change = Closure::<dyn FnMut(Event)>::new({
        let (send, input) = (send.clone(), input.clone());
        move |_: Event| send(input.files().and_then(|files| files.get(0)))
    });
    let on_cancel = Closure::<dyn FnMut(Event)>::new(move |_: Event| send(None));
    input
        .add_event_listener_with_callback("change", on_change.as_ref().unchecked_ref())
        .map_err(js_error)?;
    input
        .add_event_listener_with_callback("cancel", on_cancel.as_ref().unchecked_ref())
        .map_err(js_error)?;
    input.click();
    Ok((input, [on_change, on_cancel]))
}
