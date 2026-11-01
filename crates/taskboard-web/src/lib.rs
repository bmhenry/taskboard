//! The `taskboard-web` daemon: the HTTP API the web client runs on, which
//! [`taskboard_core::api`] describes, and the web UI itself. Commands go
//! through [`ops::execute`], as they do on the desktop, so both behave the
//! same.
//!
//! [`router`] builds the whole app, behind the [`auth`] checks that every
//! request passes through.

pub mod auth;

use std::convert::Infallible;
use std::fmt::Write;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::Json;
use axum::Router;
use axum::body::Bytes;
use axum::extract::rejection::{BytesRejection, JsonRejection, PathRejection, QueryRejection};
use axum::extract::{DefaultBodyLimit, FromRequest, FromRequestParts, Request, State};
use axum::http::{HeaderValue, StatusCode, Uri, header};
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

/// How long [`events`] waits after a change for the rest of its burst: one
/// save makes several watcher events, which should make one server event.
const SETTLE: Duration = Duration::from_millis(150);

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
    /// What every request must carry; see [`auth`].
    pub token: String,
    /// The address the daemon listens on. Requests must name it in `Host`,
    /// or any of localhost's names if it's a loopback address.
    pub addr: SocketAddr,
    /// Other names requests may give in `Host`, on `addr`'s port unless
    /// they say otherwise.
    pub allow_hosts: Vec<auth::HostName>,
}

/// The daemon's state, shared by every request.
struct Server {
    config: Config,
    /// Sent to whenever the board may have changed.
    changes: broadcast::Sender<()>,
    /// Why the store can't be watched, if it can't.
    watch_failure: Option<String>,
    /// Kept for as long as the server runs, to keep watching. It's never
    /// locked: the `Mutex` only makes it `Sync`, as axum's state must be,
    /// which not every platform's watcher is.
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
/// watching the store for [`api::EVENTS`] straight away. Every route, the
/// web UI's files included, is behind [`auth::guard`].
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
    let config = &server.config;
    let guard = auth::Guard::new(config.token.clone(), config.addr, &config.allow_hosts);
    app.layer(DefaultBodyLimit::max(MAX_BODY))
        .layer(axum::middleware::from_fn_with_state(
            Arc::new(guard),
            auth::guard,
        ))
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
    JsonBody(command): JsonBody<Command>,
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

/// The contents of `file`, one of `task`'s attachments. It's sent as a file
/// to save, never as a page the browser would show, so an attached HTML file
/// can't run scripts in the daemon's origin.
async fn file(
    State(server): Shared,
    ApiPath((task, file)): ApiPath<(Ulid, Ulid)>,
) -> Result<Response, ApiError> {
    let (name, bytes) = blocking(move || {
        let store = &server.config.store;
        let board = store.load()?;
        let attachment = board
            .get(task)
            .and_then(|t| t.files.iter().find(|f| f.id == file))
            .ok_or_else(|| not_found(format!("task {task} has no file {file}")))?;
        match std::fs::read(store.file_path(attachment)) {
            Ok(bytes) => Ok((attachment.name.clone(), bytes)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(not_found(format!(
                "{} is missing from the store",
                attachment.name
            ))),
            Err(e) => Err(taskboard_core::Error::from(e).into()),
        }
    })
    .await?;
    let mut res = bytes.into_response();
    let headers = res.headers_mut();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/octet-stream"),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    let disposition = HeaderValue::from_str(&content_disposition(&name))
        .expect("percent-encoded names are plain ASCII");
    headers.insert(header::CONTENT_DISPOSITION, disposition);
    Ok(res)
}

/// A `Content-Disposition` to save a file as `name`, which is encoded as
/// RFC 5987 says, so any name survives.
fn content_disposition(name: &str) -> String {
    let mut encoded = String::new();
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b"!#$&+-.^_`|~".contains(&b) {
            encoded.push(b as char);
        } else {
            let _ = write!(encoded, "%{b:02X}");
        }
    }
    format!("attachment; filename*=UTF-8''{encoded}")
}

/// Attaches the body, a file's raw contents, to a task.
async fn upload(
    State(server): Shared,
    ApiQuery(UploadQuery { task, name }): ApiQuery<UploadQuery>,
    Upload(body): Upload,
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

async fn path_kinds(
    JsonBody(paths): JsonBody<Vec<PathBuf>>,
) -> Result<Json<Vec<PathKind>>, ApiError> {
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
    JsonBody(settings): JsonBody<GlobalSettings>,
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

/// Server-sent [`BoardEvent`]s: [`BoardEvent::Changed`] first, then one for
/// each burst of changes to the store, or [`BoardEvent::Failed`] if it isn't
/// watched.
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
        use broadcast::error::{RecvError, TryRecvError};
        // Missed changes are covered by the reload this one causes.
        if let Err(RecvError::Closed) = changes.recv().await {
            return None;
        }
        tokio::time::sleep(SETTLE).await;
        while let Ok(()) | Err(TryRecvError::Lagged(_)) = changes.try_recv() {}
        Some((BoardEvent::Changed, changes))
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

/// [`axum::Json`] for requests, failing with an [`ApiError`].
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(ApiError))]
struct JsonBody<T>(T);

/// [`axum::extract::Query`], failing with an [`ApiError`].
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Query), rejection(ApiError))]
struct ApiQuery<T>(T);

/// [`axum::extract::Path`], failing with an [`ApiError`].
#[derive(FromRequestParts)]
#[from_request(via(axum::extract::Path), rejection(ApiError))]
struct ApiPath<T>(T);

/// An upload's body: a file's raw bytes, sent as `application/octet-stream`.
/// Requiring that type (as JSON is required elsewhere) makes browsers ask
/// the daemon before sending one from another site, with a CORS preflight.
struct Upload(Bytes);

impl<S: Send + Sync> FromRequest<S> for Upload {
    type Rejection = ApiError;

    async fn from_request(req: Request, state: &S) -> Result<Upload, ApiError> {
        let octets = req
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.split(';').next())
            .is_some_and(|t| t.trim().eq_ignore_ascii_case("application/octet-stream"));
        if !octets {
            return Err(ApiError {
                status: StatusCode::UNSUPPORTED_MEDIA_TYPE,
                message: "uploads must be sent as application/octet-stream".into(),
            });
        }
        Ok(Upload(Bytes::from_request(req, state).await?))
    }
}

/// A failed request, sent as an [`ErrorBody`].
#[derive(Debug)]
struct ApiError {
    status: StatusCode,
    message: String,
}

fn not_found(message: String) -> ApiError {
    ApiError {
        status: StatusCode::NOT_FOUND,
        message,
    }
}

/// Makes axum's rejections of a request [`ApiError`]s, keeping their status.
macro_rules! from_rejection {
    ($($rejection:ty),*) => {$(
        impl From<$rejection> for ApiError {
            fn from(r: $rejection) -> ApiError {
                ApiError {
                    status: r.status(),
                    message: r.body_text(),
                }
            }
        }
    )*};
}

from_rejection!(JsonRejection, QueryRejection, PathRejection, BytesRejection);

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

    const TOKEN: &str = "0123456789abcdef0123456789abcdef";
    const HOST: &str = "127.0.0.1:7878";

    fn fixture() -> Fixture {
        fixture_with(false, HOST, &[])
    }

    /// A fixture listening on `addr` and also taking requests for
    /// `allow_hosts`. With `web_ui`, it serves a web UI that's just an
    /// `index.html`.
    fn fixture_with(web_ui: bool, addr: &str, allow_hosts: &[&str]) -> Fixture {
        let dir = tempfile::tempdir().unwrap();
        let store = Store::open(&dir.path().join("store")).unwrap();
        let project = create_project_dir(&dir.path().join("proj")).unwrap();
        let dist = dir.path().join("dist");
        std::fs::create_dir(&dist).unwrap();
        std::fs::write(dist.join("index.html"), "<p>web UI</p>").unwrap();
        let app = router(Config {
            store: store.clone(),
            settings_path: Some(dir.path().join("config/settings.json")),
            project: project.clone(),
            named: true,
            static_dir: web_ui.then_some(dist),
            token: TOKEN.into(),
            addr: addr.parse().unwrap(),
            allow_hosts: allow_hosts.iter().map(|h| h.parse().unwrap()).collect(),
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
            self.send_as(method, uri, "application/json", body).await
        }

        /// Sends a request as a script would: to the daemon's own address,
        /// with the token as a bearer token.
        async fn send_as(&self, method: Method, uri: &str, ty: &str, body: Body) -> Response {
            let req = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, HOST)
                .header(header::AUTHORIZATION, format!("Bearer {TOKEN}"))
                .header(header::CONTENT_TYPE, ty)
                .body(body)
                .unwrap();
            self.app.clone().oneshot(req).await.unwrap()
        }

        /// GETs `uri` from `host` with only the `headers` given.
        async fn get_raw(
            &self,
            uri: &str,
            host: &str,
            headers: &[(header::HeaderName, &str)],
        ) -> Response {
            let mut req = Request::builder().uri(uri).header(header::HOST, host);
            for (name, value) in headers {
                req = req.header(name, *value);
            }
            let req = req.body(Body::empty()).unwrap();
            self.app.clone().oneshot(req).await.unwrap()
        }

        /// Uploads `bytes` as `name`, which is already percent-encoded.
        async fn upload(&self, task: Ulid, name: &str, bytes: &'static str) -> Response {
            let uri = format!("{}?task={task}&name={name}", api::UPLOADS);
            let body = Body::from(bytes);
            self.send_as(Method::POST, &uri, "application/octet-stream", body)
                .await
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

        // So do requests that can't be read.
        let res = f
            .send(Method::POST, api::COMMANDS, Body::from(r#"{"type": "#))
            .await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let ErrorBody { error } = read(res).await;
        assert!(!error.is_empty());
        let res = f.post(api::COMMANDS, json!({"type": "fly"})).await;
        assert_eq!(res.status(), StatusCode::UNPROCESSABLE_ENTITY);
        let ErrorBody { error } = read(res).await;
        assert!(error.contains("fly"), "{error}");
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
        let res = f.upload(id, "my%20notes.md", "# Notes\n").await;
        assert_eq!(res.status(), StatusCode::OK);
        let Outcome { output, .. } = read(res).await;
        let Output::File { file } = output else {
            panic!("expected File, got {output:?}");
        };
        assert_eq!(file.name, "my notes.md");

        let res = f.get(&format!("{}/{id}/{}", api::FILES, file.id)).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(
            res.headers()[header::CONTENT_TYPE],
            "application/octet-stream"
        );
        assert_eq!(
            res.headers()[header::CONTENT_DISPOSITION],
            "attachment; filename*=UTF-8''my%20notes.md"
        );
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(&bytes[..], b"# Notes\n");

        // Only a task's own files are served.
        let other = f.add("other", &f.project);
        let res = f.get(&format!("{}/{other}/{}", api::FILES, file.id)).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let unknown = Ulid::generate();
        let res = f.get(&format!("{}/{id}/{unknown}", api::FILES)).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
        let res = f.get(&format!("{}/{id}/not-an-id", api::FILES)).await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _: ErrorBody = read(res).await;
        // So is one the store has lost.
        std::fs::remove_file(f.store.file_path(&file)).unwrap();
        let res = f.get(&format!("{}/{id}/{}", api::FILES, file.id)).await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn uploads_need_plain_names_and_their_content_type() {
        let f = fixture();
        let id = f.add("t", &f.project);
        // Names are file names, not paths.
        let res = f.upload(id, "..%2Fescape", "x").await;
        assert_eq!(res.status(), StatusCode::BAD_REQUEST);
        let _: ErrorBody = read(res).await;
        // text/plain, which browsers send across sites without asking, is
        // refused.
        let uri = format!("{}?task={id}&name=a.txt", api::UPLOADS);
        let res = f
            .send_as(Method::POST, &uri, "text/plain", Body::from("x"))
            .await;
        assert_eq!(res.status(), StatusCode::UNSUPPORTED_MEDIA_TYPE);
        let _: ErrorBody = read(res).await;
        assert!(f.store.load().unwrap().tasks[0].files.is_empty());
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
        let changed = format!("data: {}\n\n", json(&BoardEvent::Changed));
        // One on connecting, then just one for a change made elsewhere,
        // though saving it makes several watcher events.
        let wait = Duration::from_secs(5);
        assert_eq!(next_event(&mut body, wait).await.unwrap(), changed);
        f.add("from tb", &f.project);
        assert_eq!(next_event(&mut body, wait).await.unwrap(), changed);
        assert_eq!(next_event(&mut body, SETTLE * 4).await, None);
    }

    /// The next event `body` sends within `wait`, if any.
    async fn next_event(body: &mut Body, wait: Duration) -> Option<String> {
        let frame = tokio::time::timeout(wait, body.frame()).await.ok()?;
        let data = frame.unwrap().unwrap().into_data().unwrap();
        Some(String::from_utf8(data.to_vec()).unwrap())
    }

    fn static_fixture() -> Fixture {
        fixture_with(true, HOST, &[])
    }

    async fn text(res: Response) -> String {
        let bytes = res.into_body().collect().await.unwrap().to_bytes();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    #[tokio::test]
    async fn everything_needs_the_token() {
        let f = static_fixture();
        let id = f.add("t", &f.project);
        let file = format!("{}/{id}/{}", api::FILES, Ulid::generate());
        for uri in [api::BOARD, api::EVENTS, api::SETTINGS, &file] {
            let res = f.get_raw(uri, HOST, &[]).await;
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{uri}");
            let ErrorBody { error } = read(res).await;
            assert!(error.contains("token"), "{error}");
        }
        // Pages say where to find it.
        for uri in ["/", "/index.html", "/nowhere"] {
            let res = f.get_raw(uri, HOST, &[]).await;
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{uri}");
            assert!(text(res).await.contains("printed"));
        }
        // Commands too.
        let req = Request::builder()
            .method(Method::POST)
            .uri(api::COMMANDS)
            .header(header::HOST, HOST)
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(json!({"type": "delete", "id": id}).to_string()))
            .unwrap();
        let res = f.app.clone().oneshot(req).await.unwrap();
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert_eq!(f.store.load().unwrap().tasks.len(), 1);
    }

    #[tokio::test]
    async fn wrong_tokens_are_refused() {
        let f = static_fixture();
        let wrong = "0123456789abcdef0123456789abcdee";
        let bearer = format!("Bearer {wrong}");
        let cookie = format!("tb_token={wrong}");
        // Right but cut short, and right but in the wrong cookie.
        let short = format!("Bearer {}", &TOKEN[..16]);
        let elsewhere = format!("other={TOKEN}");
        for headers in [
            [(header::AUTHORIZATION, bearer.as_str())],
            [(header::COOKIE, cookie.as_str())],
            [(header::AUTHORIZATION, short.as_str())],
            [(header::COOKIE, elsewhere.as_str())],
        ] {
            let res = f.get_raw(api::BOARD, HOST, &headers).await;
            assert_eq!(res.status(), StatusCode::UNAUTHORIZED, "{headers:?}");
        }
        let res = f.get_raw(&format!("/?token={wrong}"), HOST, &[]).await;
        assert_eq!(res.status(), StatusCode::UNAUTHORIZED);
        assert!(!res.headers().contains_key(header::SET_COOKIE));
    }

    #[tokio::test]
    async fn the_token_link_sets_a_cookie_that_then_works() {
        let f = static_fixture();
        let res = f.get_raw(&format!("/?token={TOKEN}"), HOST, &[]).await;
        assert_eq!(res.status(), StatusCode::SEE_OTHER);
        assert_eq!(res.headers()[header::LOCATION], "/");
        let cookie = res.headers()[header::SET_COOKIE].to_str().unwrap();
        assert!(
            cookie.starts_with(&format!("tb_token={TOKEN};")),
            "{cookie}"
        );
        for attr in ["HttpOnly", "SameSite=Strict", "Path=/"] {
            assert!(cookie.split("; ").any(|a| a == attr), "{cookie}");
        }
        // The rest of the query stays.
        let uri = format!("/index.html?a=1&token={TOKEN}&b=2");
        let res = f.get_raw(&uri, HOST, &[]).await;
        assert_eq!(res.headers()[header::LOCATION], "/index.html?a=1&b=2");

        let cookie = format!("theme=dark; tb_token={TOKEN}");
        let with_cookie = [(header::COOKIE, cookie.as_str())];
        let res = f.get_raw("/", HOST, &with_cookie).await;
        assert_eq!(res.status(), StatusCode::OK);
        assert_eq!(text(res).await, "<p>web UI</p>");
        let res = f.get_raw(api::BOARD, HOST, &with_cookie).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = f.get_raw(api::EVENTS, HOST, &with_cookie).await;
        assert_eq!(res.status(), StatusCode::OK);
        let bearer = format!("Bearer {TOKEN}");
        let res = f
            .get_raw(api::BOARD, HOST, &[(header::AUTHORIZATION, &bearer)])
            .await;
        assert_eq!(res.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn requests_must_name_the_daemon() {
        let f = fixture();
        let bearer = format!("Bearer {TOKEN}");
        let auth = [(header::AUTHORIZATION, bearer.as_str())];
        for host in [
            "127.0.0.1:7878",
            "localhost:7878",
            "LocalHost:7878",
            "[::1]:7878",
        ] {
            let res = f.get_raw(api::BOARD, host, &auth).await;
            assert_eq!(res.status(), StatusCode::OK, "{host}");
        }
        // A rebound name, the wrong port, no port (so 80), and nonsense are
        // turned away, token or no token, and before the token is checked.
        for host in [
            "evil.example:7878",
            "127.0.0.1:9999",
            "localhost",
            "a b",
            "",
        ] {
            let res = f.get_raw(api::BOARD, host, &auth).await;
            assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST, "{host}");
            let _: ErrorBody = read(res).await;
            let uri = format!("/?token={TOKEN}");
            let res = f.get_raw(&uri, host, &[]).await;
            assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST, "{host}");
            assert!(!res.headers().contains_key(header::SET_COOKIE));
        }
    }

    #[tokio::test]
    async fn other_hosts_can_be_allowed() {
        let f = fixture_with(false, HOST, &["box.tailnet.ts.net", "localhost:9000"]);
        let bearer = format!("Bearer {TOKEN}");
        let auth = [(header::AUTHORIZATION, bearer.as_str())];
        for host in [
            "box.tailnet.ts.net:7878",
            "localhost:9000",
            "localhost:7878",
        ] {
            let res = f.get_raw(api::BOARD, host, &auth).await;
            assert_eq!(res.status(), StatusCode::OK, "{host}");
        }
        for host in [
            "box.tailnet.ts.net:9000",
            "box.tailnet.ts.net",
            "127.0.0.1:9000",
        ] {
            let res = f.get_raw(api::BOARD, host, &auth).await;
            assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST, "{host}");
        }

        // Off loopback, only the address itself (and what's allowed) goes.
        let f = fixture_with(false, "192.168.1.5:7878", &[]);
        let res = f.get_raw(api::BOARD, "192.168.1.5:7878", &auth).await;
        assert_eq!(res.status(), StatusCode::OK);
        let res = f.get_raw(api::BOARD, "localhost:7878", &auth).await;
        assert_eq!(res.status(), StatusCode::MISDIRECTED_REQUEST);
    }
}
