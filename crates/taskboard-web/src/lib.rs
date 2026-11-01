//! The `taskboard-web` daemon: the HTTP API the web client runs on, which
//! [`taskboard_core::api`] describes, and the web UI itself. Commands go
//! through [`ops::execute`], as they do on the desktop, so both behave the
//! same.
//!
//! [`router`] builds the whole app. Anything every request should pass
//! through, like authentication, can go on it as a `.layer`.

use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, Path, Query, State};
use axum::http::{StatusCode, Uri, header};
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{get, post};
use futures_util::{Stream, StreamExt, stream};
use taskboard_core::api::{
    self, BoardEvent, BoardResponse, ErrorBody, PathKind, SettingsResponse, UploadQuery,
};
use taskboard_core::ops::{self, Command, FileSource, Outcome};
use taskboard_core::{Board, GlobalSettings, Store, Ulid, watch};
use tokio::sync::broadcast;
use tower_http::services::ServeDir;

/// The largest request body taken, which bounds uploads: attaching a file in
/// a command sends it as base64, a third bigger again.
pub const MAX_BODY: usize = 64 * 1024 * 1024;

/// How the daemon is set up.
pub struct Config {
    pub store: Store,
    /// Where global settings are saved; `None` uses the defaults and can't
    /// save them.
    pub settings_path: Option<PathBuf>,
    /// The project new tasks go in by default, when it is one; see
    /// [`Config::named`].
    pub project: PathBuf,
    /// Whether `project` was named on the command line, which makes it the
    /// default even if it isn't a git repository or on the board yet.
    pub named: bool,
    /// A directory to serve the web UI from (trunk's `dist/`); without it a
    /// placeholder page is served.
    pub static_dir: Option<PathBuf>,
}

/// The daemon's state, shared by every request.
struct Server {
    config: Config,
    /// Sent to whenever the board may have changed.
    changes: broadcast::Sender<()>,
    /// Why the store can't be watched, if it can't.
    watch_failure: Option<String>,
    /// Kept for as long as the server runs, to keep watching.
    _watcher: Option<Mutex<Box<dyn Send>>>,
}

impl Server {
    fn new(config: Config) -> Server {
        // A client this far behind just reloads; see `events`.
        let (changes, _) = broadcast::channel(16);
        let tx = changes.clone();
        let store = &config.store;
        let watched = watch::tasks(store.dir(), &store.tasks_path(), move || {
            // Fails only when no one is listening.
            let _ = tx.send(());
        });
        let (watcher, watch_failure) = match watched {
            Ok(w) => (Some(Mutex::new(Box::new(w) as Box<dyn Send>)), None),
            Err(e) => {
                let why = format!("couldn't watch {}: {e}", store.dir().display());
                eprintln!("taskboard-web: {why}; live updates are off");
                (None, Some(why))
            }
        };
        Server {
            config,
            changes,
            watch_failure,
            _watcher: watcher,
        }
    }

    /// The project new tasks go in by default, as the desktop app picks it:
    /// a daemon started somewhere that isn't a project (like `~`) has none.
    fn default_project(&self, board: &Board) -> Option<PathBuf> {
        let project = &self.config.project;
        let is_project =
            self.config.named || project.join(".git").exists() || board.project(project).is_some();
        is_project.then(|| project.clone())
    }

    fn load_settings(&self) -> taskboard_core::Result<GlobalSettings> {
        match &self.config.settings_path {
            Some(path) => GlobalSettings::load(path),
            None => Ok(GlobalSettings::default()),
        }
    }

    /// Runs `command` with the hooks in the settings as they are now, since
    /// the desktop app or `tb` may have changed them.
    fn run(&self, command: Command) -> Result<Outcome, ApiError> {
        let settings = self.load_settings().unwrap_or_else(|e| {
            eprintln!(
                "taskboard-web: warning: using default settings (hooks off, new tasks at top): {e}"
            );
            GlobalSettings::default()
        });
        Ok(ops::execute(&self.config.store, &settings, command)?)
    }
}

type Shared = State<Arc<Server>>;

/// The daemon's routes, over the store and settings in `config`. It starts
/// watching the store for [`api::EVENTS`] straight away.
pub fn router(config: Config) -> Router {
    let server = Arc::new(Server::new(config));
    let app = Router::new()
        .route(api::BOARD, get(board))
        .route(api::COMMANDS, post(command))
        .route(&format!("{}/{{task}}/{{file}}", api::FILES), get(file))
        .route(api::UPLOADS, post(upload))
        .route(api::PATH_KINDS, post(path_kinds))
        .route(api::SETTINGS, get(settings).put(save_settings))
        .route(api::EVENTS, get(events));
    let app = match &server.config.static_dir {
        Some(dir) => app.fallback_service(ServeDir::new(dir)),
        None => app.fallback(placeholder),
    };
    app.layer(DefaultBodyLimit::max(MAX_BODY))
        .with_state(server)
}

async fn board(State(server): Shared) -> Result<Json<BoardResponse>, ApiError> {
    blocking(move || {
        let board = server.config.store.load()?;
        let missing_projects = board
            .projects
            .iter()
            .filter(|p| PathKind::of(&p.path) != PathKind::Dir)
            .map(|p| p.path.clone())
            .collect();
        let default_project = server.default_project(&board);
        Ok(Json(BoardResponse {
            board,
            missing_projects,
            default_project,
        }))
    })
    .await
}

async fn command(
    State(server): Shared,
    Json(command): Json<Command>,
) -> Result<Json<Outcome>, ApiError> {
    if !command.is_remote_safe() {
        return Err(ApiError {
            status: StatusCode::FORBIDDEN,
            message: "files on the server can't be attached from the web; upload them instead"
                .into(),
        });
    }
    blocking(move || server.run(command).map(Json)).await
}

/// The contents of `file`, one of `task`'s attachments. It's sent as bytes
/// to save, never as a page the browser would show, so an attached HTML file
/// can't run scripts in the daemon's origin.
async fn file(
    State(server): Shared,
    Path((task, file)): Path<(Ulid, Ulid)>,
) -> Result<Response, ApiError> {
    let bytes = blocking(move || {
        let store = &server.config.store;
        let board = store.load()?;
        let attachment = board
            .get(task)
            .and_then(|t| t.files.iter().find(|f| f.id == file))
            .ok_or_else(|| ApiError {
                status: StatusCode::NOT_FOUND,
                message: format!("task {task} has no file {file}"),
            })?;
        Ok(std::fs::read(store.file_path(attachment)).map_err(taskboard_core::Error::from)?)
    })
    .await?;
    let headers = [
        (header::CONTENT_TYPE, "application/octet-stream"),
        (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
    ];
    Ok((headers, bytes).into_response())
}

/// Attaches the body, a file's raw contents, to a task.
async fn upload(
    State(server): Shared,
    Query(UploadQuery { task, name }): Query<UploadQuery>,
    body: Bytes,
) -> Result<Json<Outcome>, ApiError> {
    let command = Command::Attach {
        task,
        source: FileSource::Bytes {
            name,
            bytes: body.to_vec(),
        },
    };
    blocking(move || server.run(command).map(Json)).await
}

async fn path_kinds(Json(paths): Json<Vec<PathBuf>>) -> Result<Json<Vec<PathKind>>, ApiError> {
    blocking(move || Ok(Json(paths.iter().map(|p| PathKind::of(p)).collect()))).await
}

async fn settings(State(server): Shared) -> Result<Json<SettingsResponse>, ApiError> {
    blocking(move || {
        Ok(Json(SettingsResponse {
            settings: server.load_settings()?,
            location: (server.config.settings_path.as_ref()).map(|p| p.display().to_string()),
        }))
    })
    .await
}

async fn save_settings(
    State(server): Shared,
    Json(settings): Json<GlobalSettings>,
) -> Result<StatusCode, ApiError> {
    blocking(move || {
        let path = server
            .config
            .settings_path
            .as_ref()
            .ok_or_else(|| ApiError {
                status: StatusCode::INTERNAL_SERVER_ERROR,
                message: "nowhere to save settings: neither XDG_CONFIG_HOME nor HOME is set".into(),
            })?;
        settings.save(path)?;
        Ok(StatusCode::NO_CONTENT)
    })
    .await
}

/// Server-sent [`BoardEvent`]s: [`BoardEvent::Changed`] first, then for each
/// change to the store, or [`BoardEvent::Failed`] if it isn't watched.
async fn events(State(server): Shared) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let first = match &server.watch_failure {
        Some(reason) => BoardEvent::Failed {
            reason: reason.clone(),
        },
        None => BoardEvent::Changed,
    };
    // Subscribed before the first event goes, so no change slips between.
    // Without a watcher nothing is ever sent, so the stream stays open and
    // the browser doesn't keep reconnecting.
    let changes = server.changes.subscribe();
    let rest = stream::unfold(changes, |mut changes| async move {
        match changes.recv().await {
            // Missed changes are covered by the reload this one causes.
            Ok(()) | Err(broadcast::error::RecvError::Lagged(_)) => {
                Some((BoardEvent::Changed, changes))
            }
            Err(broadcast::error::RecvError::Closed) => None,
        }
    });
    let events = stream::once(async { first })
        .chain(rest)
        .map(|event| Ok(Event::default().data(json(&event))));
    Sse::new(events).keep_alive(KeepAlive::default())
}

/// Stands in for the web UI until it's built into the daemon.
async fn placeholder(uri: Uri) -> Response {
    if uri.path() != "/" {
        return StatusCode::NOT_FOUND.into_response();
    }
    Html(concat!(
        "<!doctype html><title>Taskboard</title>",
        "<p>taskboard-web is running, but has no web UI to serve. ",
        "Start it with <code>--static DIR</code>, DIR being trunk's <code>dist/</code>.</p>",
    ))
    .into_response()
}

/// Runs `f`, which uses the filesystem, off the async workers.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, ApiError> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f).await.map_err(|e| ApiError {
        status: StatusCode::INTERNAL_SERVER_ERROR,
        message: e.to_string(),
    })?
}

fn json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("API types serialize")
}

/// A failed request, sent as an [`ErrorBody`].
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

impl From<taskboard_core::Error> for ApiError {
    fn from(e: taskboard_core::Error) -> ApiError {
        use taskboard_core::Error;
        let status = match &e {
            Error::NotFound(_) => StatusCode::NOT_FOUND,
            Error::Ambiguous(..) | Error::Invalid(_) => StatusCode::BAD_REQUEST,
            Error::Io(_) | Error::Json(_) | Error::UnsupportedSchema(_) => {
                StatusCode::INTERNAL_SERVER_ERROR
            }
        };
        ApiError {
            status,
            message: e.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let body = ErrorBody {
            error: self.message,
        };
        (self.status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use axum::body::Body;
    use axum::http::{Method, Request};
    use http_body_util::BodyExt;
    use serde::de::DeserializeOwned;
    use serde_json::json;
    use taskboard_core::{Output, Status, create_project_dir};
    use tower::ServiceExt;

    use super::*;

    struct Fixture {
        dir: tempfile::TempDir,
        store: Store,
        app: Router,
        /// The default project, which exists, canonical.
        project: PathBuf,
    }

    fn fixture() -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let project = create_project_dir(&dir.path().join("proj")).unwrap();
        let app = router(Config {
            store: store.clone(),
            settings_path: Some(dir.path().join("config/settings.json")),
            project: project.clone(),
            named: true,
            static_dir: None,
        });
        Fixture {
            dir,
            store,
            app,
            project,
        }
    }

    impl Fixture {
        async fn send(&self, method: Method, uri: &str, body: Body) -> Response {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::CONTENT_TYPE, "application/json")
                .body(body)
                .unwrap();
            self.app.clone().oneshot(req).await.unwrap()
        }

        async fn get(&self, uri: &str) -> Response {
            self.send(Method::GET, uri, Body::empty()).await
        }

        async fn post(&self, uri: &str, body: serde_json::Value) -> Response {
            self.send(Method::POST, uri, Body::from(body.to_string()))
                .await
        }

        /// Adds a task in `project` straight to the store.
        fn add(&self, title: &str, project: &std::path::Path) -> Ulid {
            let command = Command::Create {
                title: title.into(),
                project: project.display().to_string(),
                description: String::new(),
                status: None,
                links: Vec::new(),
                files: Vec::new(),
                position: None,
                epic: false,
                parent: None,
                id: None,
            };
            match ops::execute(&self.store, &GlobalSettings::default(), command)
                .unwrap()
                .output
            {
                Output::Created { id } => id,
                other => panic!("expected Created, got {other:?}"),
            }
        }
    }

    async fn read<T: DeserializeOwned>(res: Response) -> T {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        serde_json::from_slice(&bytes).unwrap()
    }

    #[tokio::test]
    async fn board_has_missing_projects_and_the_default_project() {
        let f = fixture();
        f.add("here", &f.project);
        let gone = create_project_dir(&f.dir.path().join("gone")).unwrap();
        f.add("gone", &gone);
        std::fs::remove_dir(&gone).unwrap();

        let res = f.get(api::BOARD).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res: BoardResponse = read(res).await;
        assert_eq!(res.board.tasks.len(), 2);
        assert_eq!(res.missing_projects, vec![gone]);
        assert_eq!(res.default_project, Some(f.project.clone()));
    }

    #[tokio::test]
    async fn commands_change_the_store() {
        let f = fixture();
        let project = f.project.display().to_string();
        let res = f
            .post(
                api::COMMANDS,
                json!({"type": "create", "title": "Web task", "project": project}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        let Outcome { output, events, .. } = read(res).await;
        let Output::Created { id } = output else {
            panic!("expected Created, got {output:?}");
        };
        assert_eq!(events.len(), 1);

        let res = f
            .post(
                api::COMMANDS,
                json!({"type": "move", "id": id, "status": "closed"}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            f.store.load().unwrap().get(id).unwrap().status,
            Status::Closed
        );

        // Errors from the store come back with a status and a message.
        let res = f
            .post(
                api::COMMANDS,
                json!({"type": "delete", "id": Ulid::generate()}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let ErrorBody { error } = read(res).await;
        assert!(error.contains("no task"), "{error}");
    }

    #[tokio::test]
    async fn attaching_server_paths_is_refused() {
        let f = fixture();
        let id = f.add("t", &f.project);
        let secret = f.dir.path().join("secret.txt");
        std::fs::write(&secret, "private").unwrap();
        let path = json!({"type": "path", "path": secret});

        let res = f
            .post(
                api::COMMANDS,
                json!({"type": "attach", "task": id, "source": path}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);
        let project = f.project.display().to_string();
        let res = f
            .post(
                api::COMMANDS,
                json!({"type": "create", "title": "u", "project": project, "files": [path]}),
            )
            .await;
        assert_eq!(res.status(), StatusCode::FORBIDDEN);

        let board = f.store.load().unwrap();
        assert_eq!(board.tasks.len(), 1);
        assert!(board.tasks[0].files.is_empty());
    }

    #[tokio::test]
    async fn uploads_attach_and_files_serve_them() {
        let f = fixture();
        let id = f.add("t", &f.project);
        let uri = format!("{}?task={id}&name=notes.md", api::UPLOADS);
        let res = f.send(Method::POST, &uri, Body::from("# Notes\n")).await;
        assert_eq!(res.status(), StatusCode::OK);
        let Outcome { output, .. } = read(res).await;
        let Output::File { file } = output else {
            panic!("expected File, got {output:?}");
        };
        assert_eq!(file.name, "notes.md");

        let res = f.get(&format!("{}/{id}/{}", api::FILES, file.id)).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"# Notes\n");

        // Only a task's own files are served.
        let other = f.add("other", &f.project);
        let res = f.get(&format!("{}/{other}/{}", api::FILES, file.id)).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        // Names are file names, not paths.
        let uri = format!("{}?task={id}&name=..%2Fescape", api::UPLOADS);
        let res = f.send(Method::POST, &uri, Body::from("x")).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
    }

    #[tokio::test]
    async fn settings_round_trip() {
        let f = fixture();
        let SettingsResponse { settings, location } = read(f.get(api::SETTINGS).await).await;
        assert_eq!(settings, GlobalSettings::default());
        assert!(location.unwrap().ends_with("settings.json"));

        let changed = GlobalSettings {
            collapse_empty_columns: false,
            ..GlobalSettings::default()
        };
        let body = Body::from(serde_json::to_string(&changed).unwrap());
        let res = f.send(Method::PUT, api::SETTINGS, body).await;
        assert_eq!(res.status(), StatusCode::NO_CONTENT);
        let res: SettingsResponse = read(f.get(api::SETTINGS).await).await;
        assert_eq!(res.settings, changed);
    }

    #[tokio::test]
    async fn path_kinds_describe_the_server() {
        let f = fixture();
        let file = f.dir.path().join("file");
        std::fs::write(&file, "").unwrap();
        let missing = f.dir.path().join("missing");
        let res = f
            .post(api::PATH_KINDS, json!([f.project, file, missing]))
            .await;
        let kinds: Vec<PathKind> = read(res).await;
        assert_eq!(kinds, [PathKind::Dir, PathKind::File, PathKind::Missing]);
    }

    #[tokio::test]
    async fn events_report_changes_to_the_store() {
        let f = fixture();
        let res = f.get(api::EVENTS).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(res.headers()[header::CONTENT_TYPE], "text/event-stream");
        let mut body = res.into_body();
        let mut next = async || {
            let frame = tokio::time::timeout(Duration::from_secs(5), body.frame())
                .await
                .expect("an event within 5s")
                .unwrap()
                .unwrap();
            String::from_utf8(frame.into_data().unwrap().to_vec()).unwrap()
        };
        let changed = format!("data: {}\n\n", json(&BoardEvent::Changed));
        // One on connecting, then one for a change made elsewhere.
        assert_eq!(next().await, changed);
        f.add("from tb", &f.project);
        assert_eq!(next().await, changed);
    }
}
