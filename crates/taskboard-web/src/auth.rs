//! Keeping the daemon to its owner. Hooks run commands and files can be
//! written through it, so every request must carry the daemon's token, and
//! must be addressed to the daemon by a name it answers to.
//!
//! The token comes as a cookie, which `?token=` on any page sets (the link
//! the daemon prints on starting has it), or as `Authorization: Bearer`.
//! Checking the `Host` header stops DNS rebinding: a page from another site
//! that makes its name resolve to 127.0.0.1 still sends that name, so it's
//! turned away before its requests reach the API.

use std::fmt;
use std::io::{self, Write};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::sync::Arc;

use axum::extract::{Request, State};
use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{Html, IntoResponse, Redirect, Response};

use crate::ApiError;

/// The cookie the token is kept in.
pub const COOKIE: &str = "tb_token";

/// The query parameter that hands the browser the token.
const PARAM: &str = "token";

/// Random bytes in a token: 256 bits.
const TOKEN_BYTES: usize = 32;

/// Where the token is kept: `$XDG_CONFIG_HOME/tb/web-token`, or
/// `~/.config/tb/web-token`. `None` if neither variable is set.
pub fn token_path() -> Option<PathBuf> {
    Some(taskboard_core::config_dir()?.join("web-token"))
}

/// The token in `path`, made there first (readable only by its owner) if
/// there isn't one yet, so it stays the same from one run to the next.
pub fn load_or_create_token(path: &Path) -> io::Result<String> {
    if let Some(token) = read_token(path)? {
        return Ok(token);
    }
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut bytes = [0; TOKEN_BYTES];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    let token: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    match options.open(path) {
        Ok(mut file) => {
            file.write_all(format!("{token}\n").as_bytes())?;
            Ok(token)
        }
        // Another daemon starting at the same time made one first.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read_token(path)?
            .ok_or_else(|| io::Error::other(format!("{} is empty", path.display()))),
        Err(e) => Err(e),
    }
}

/// The token in `path`, if it exists and has one.
fn read_token(path: &Path) -> io::Result<Option<String>> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(Some(text.trim().to_owned()).filter(|t| !t.is_empty())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

/// A name the daemon may be reached by, as `--allow-host` takes it: a host
/// name or IP address (IPv6 in brackets), with a port or without one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostName {
    name: String,
    port: Option<u16>,
}

impl FromStr for HostName {
    type Err = String;

    fn from_str(s: &str) -> Result<HostName, String> {
        let s = s.to_ascii_lowercase();
        let (name, port) = match s.strip_prefix('[') {
            Some(rest) => {
                let (inside, after) = rest.split_once(']').ok_or("missing ']'")?;
                let port = match after {
                    "" => None,
                    _ => Some(after.strip_prefix(':').ok_or("expected ':' after ']'")?),
                };
                (format!("[{inside}]"), port)
            }
            None => match s.split_once(':') {
                Some((name, port)) => (name.to_owned(), Some(port)),
                None => (s.clone(), None),
            },
        };
        let port = port
            .map(|p| p.parse().map_err(|_| format!("bad port {p:?}")))
            .transpose()?;
        let plain = |c: char| c.is_ascii_alphanumeric() || "-._:".contains(c);
        let inner = name.trim_start_matches('[').trim_end_matches(']');
        if inner.is_empty() || !inner.chars().all(plain) || (inner.contains(':') && inner == name) {
            return Err(format!("{s:?} isn't a host name or [IPv6] address"));
        }
        Ok(HostName { name, port })
    }
}

impl fmt::Display for HostName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.port {
            Some(port) => write!(f, "{}:{port}", self.name),
            None => f.write_str(&self.name),
        }
    }
}

/// What every request is checked against.
pub(crate) struct Guard {
    token: String,
    /// The `Host`s requests may name, each with its port.
    hosts: Vec<HostName>,
}

impl Guard {
    /// Takes requests for `token`, addressed to `addr` (all of localhost's
    /// names if it's a loopback address) or to any of `allowed`, which are
    /// on `addr`'s port unless they say otherwise.
    pub(crate) fn new(token: String, addr: SocketAddr, allowed: &[HostName]) -> Guard {
        let port = Some(addr.port());
        let mut hosts = if addr.ip().is_loopback() {
            ["127.0.0.1", "localhost", "[::1]"]
                .map(|name| HostName {
                    name: name.into(),
                    port,
                })
                .to_vec()
        } else {
            vec![addr.to_string().parse().expect("socket addresses parse")]
        };
        hosts.extend(allowed.iter().map(|h| HostName {
            name: h.name.clone(),
            port: h.port.or(port),
        }));
        Guard { token, hosts }
    }

    fn host_allowed(&self, headers: &HeaderMap) -> bool {
        let host = headers.get(header::HOST).and_then(|v| v.to_str().ok());
        let Some(Ok(mut host)) = host.map(HostName::from_str) else {
            return false;
        };
        // A browser leaves out the port when it's HTTP's own.
        host.port = host.port.or(Some(80));
        self.hosts.contains(&host)
    }

    fn is_token(&self, candidate: &str) -> bool {
        same(candidate.as_bytes(), self.token.as_bytes())
    }

    /// Whether `headers` carry the token, as a cookie or a bearer token.
    fn authorized(&self, headers: &HeaderMap) -> bool {
        let bearer = headers
            .get_all(header::AUTHORIZATION)
            .iter()
            .filter_map(|v| v.to_str().ok()?.strip_prefix("Bearer "));
        let cookies = headers
            .get_all(header::COOKIE)
            .iter()
            .filter_map(|v| v.to_str().ok())
            .flat_map(|v| v.split(';'))
            .filter_map(|c| c.trim().strip_prefix(COOKIE)?.strip_prefix('='));
        bearer.chain(cookies).any(|t| self.is_token(t.trim()))
    }
}

/// Whether `a` and `b` are the same, taking as long to say so wherever they
/// differ, so the token can't be guessed a byte at a time.
fn same(a: &[u8], b: &[u8]) -> bool {
    let diff = a
        .iter()
        .zip(b)
        .fold(0, |acc, (x, y)| std::hint::black_box(acc | (x ^ y)));
    a.len() == b.len() && diff == 0
}

/// The middleware: checks `Host`, then the token. A page loaded with the
/// right `?token=` sets the cookie and sends the browser back to the page
/// without it, so the token doesn't stay in its address bar or history.
pub(crate) async fn guard(State(guard): State<Arc<Guard>>, req: Request, next: Next) -> Response {
    if !guard.host_allowed(req.headers()) {
        return ApiError {
            status: StatusCode::MISDIRECTED_REQUEST,
            message: "this daemon isn't served under that name; see --allow-host".into(),
        }
        .into_response();
    }
    let uri = req.uri();
    let page = matches!(*req.method(), Method::GET | Method::HEAD);
    if let (true, Some(query)) = (page, uri.query()) {
        let (tokens, rest): (Vec<_>, Vec<_>) = query
            .split('&')
            .partition(|pair| pair.split('=').next() == Some(PARAM));
        if !tokens.is_empty() {
            let valid = tokens
                .iter()
                .all(|t| t.split_once('=').is_some_and(|(_, v)| guard.is_token(v)));
            if !valid {
                return unauthorized(uri.path());
            }
            let mut to = uri.path().to_owned();
            if !rest.is_empty() {
                to = format!("{to}?{}", rest.join("&"));
            }
            let cookie = format!(
                "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000",
                guard.token
            );
            let mut res = Redirect::to(&to).into_response();
            res.headers_mut().insert(
                header::SET_COOKIE,
                HeaderValue::from_str(&cookie).expect("tokens are plain ASCII"),
            );
            return res;
        }
    }
    if !guard.authorized(req.headers()) {
        return unauthorized(uri.path());
    }
    next.run(req).await
}

/// The API's own error for its routes; for pages, one a person can read.
fn unauthorized(path: &str) -> Response {
    let mut res = if path == "/api" || path.starts_with("/api/") {
        ApiError {
            status: StatusCode::UNAUTHORIZED,
            message: "missing or wrong token: send the tb_token cookie or a bearer token".into(),
        }
        .into_response()
    } else {
        let page = Html(concat!(
            "<!doctype html><title>Taskboard: not signed in</title>",
            "<p>This page needs the daemon's token. Open the link ",
            "<code>taskboard-web</code> printed when it started, the one ending ",
            "in <code>?token=…</code>.</p>",
        ));
        (StatusCode::UNAUTHORIZED, page).into_response()
    };
    res.headers_mut()
        .insert(header::WWW_AUTHENTICATE, HeaderValue::from_static("Bearer"));
    res
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_names_parse() {
        let host = |s: &str| s.parse::<HostName>();
        assert_eq!(
            host("Box.Tail-Net.ts.net").unwrap().to_string(),
            "box.tail-net.ts.net"
        );
        assert_eq!(host("localhost:9000").unwrap().port, Some(9000));
        assert_eq!(host("[::1]:7878").unwrap().to_string(), "[::1]:7878");
        assert_eq!(host("[fe80::1]").unwrap().port, None);
        for bad in ["", ":80", "a:b", "::1", "[::1", "[::1]80", "a/b", "a b"] {
            assert!(host(bad).is_err(), "{bad:?} parsed");
        }
    }

    #[test]
    fn same_compares_whole_strings() {
        assert!(same(b"abc", b"abc"));
        assert!(!same(b"abc", b"abd"));
        assert!(!same(b"abc", b"ab"));
        assert!(!same(b"", b"a"));
    }

    #[test]
    fn tokens_are_made_once_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tb/web-token");
        let token = load_or_create_token(&path).unwrap();
        assert_eq!(token.len(), TOKEN_BYTES * 2);
        assert!(token.bytes().all(|b| b.is_ascii_hexdigit()));
        assert_eq!(load_or_create_token(&path).unwrap(), token);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // Each place gets its own.
        let other = load_or_create_token(&dir.path().join("other")).unwrap();
        assert_ne!(other, token);
    }
}
