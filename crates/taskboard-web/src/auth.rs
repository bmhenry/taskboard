//! Keeping the daemon to its owner. Hooks run commands and files can be
//! written through it, so every request must carry the daemon's token, and
//! must be addressed to the daemon by a name it answers to.
//!
//! The token comes as a cookie, which `?token=` on any page sets (the link
//! the daemon prints on starting has it), or as `Authorization: Bearer`.
//! Checking the `Host` header stops DNS rebinding: a page from another site
//! that makes its name resolve to 127.0.0.1 still sends that name, so it's
//! turned away before its requests reach the API. Checking `Origin` on
//! anything but a read stops other sites posting to it with the cookie.

use std::fmt;
use std::io::{self, Write};
use std::net::{IpAddr, SocketAddr};
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

/// Random bytes in a token: 256 bits, written as twice as many hex digits.
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
    let dir = match path.parent() {
        Some(dir) if !dir.as_os_str().is_empty() => dir,
        _ => Path::new("."),
    };
    create_private_dir(dir)?;
    let token = hex(&random::<TOKEN_BYTES>()?);
    // Written in full to a file of its own and then linked into place, so
    // the token file is never there half-written, even to another daemon
    // starting at the same time or after a crash.
    let temp = dir.join(format!(".web-token.{}", hex(&random::<8>()?)));
    let linked = write_private(&temp, &format!("{token}\n")).and_then(|()| {
        let linked = std::fs::hard_link(&temp, path);
        let _ = std::fs::remove_file(&temp);
        linked
    });
    match linked {
        Ok(()) => Ok(token),
        // Another daemon starting at the same time made one first.
        Err(e) if e.kind() == io::ErrorKind::AlreadyExists => read_token(path)?
            .ok_or_else(|| io::Error::other(format!("{} vanished", path.display()))),
        Err(e) => {
            let _ = std::fs::remove_file(&temp);
            Err(e)
        }
    }
}

/// The token in `path`, if there's a file there. Anything but a regular
/// file holding a token is an error, which says how to start over.
fn read_token(path: &Path) -> io::Result<Option<String>> {
    let shown = path.display();
    let invalid = |why: &str| {
        io::Error::new(
            io::ErrorKind::InvalidData,
            format!("{shown} {why}; delete it to make a new token"),
        )
    };
    let meta = match std::fs::symlink_metadata(path) {
        Ok(meta) => meta,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if !meta.file_type().is_file() {
        return Err(invalid("isn't a regular file"));
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = meta.permissions().mode();
        if mode & 0o077 != 0 {
            eprintln!(
                "taskboard-web: warning: {shown} could be read by others (mode {:o}); making it 600",
                mode & 0o777
            );
            std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
        }
    }
    let text = std::fs::read_to_string(path)?;
    let token = text.trim();
    if token.len() != TOKEN_BYTES * 2 || !token.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(invalid(&format!(
            "doesn't hold a token ({} hex digits)",
            TOKEN_BYTES * 2
        )));
    }
    Ok(Some(token.to_owned()))
}

/// Makes `dir` and any missing parents, readable only by their owner.
fn create_private_dir(dir: &Path) -> io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)
}

/// Writes `text` to a new file at `path`, readable only by its owner, and
/// waits for it to reach the disk.
fn write_private(path: &Path, text: &str) -> io::Result<()> {
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(text.as_bytes())?;
    file.sync_all()
}

fn random<const N: usize>() -> io::Result<[u8; N]> {
    let mut bytes = [0; N];
    getrandom::fill(&mut bytes).map_err(|e| io::Error::other(e.to_string()))?;
    Ok(bytes)
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
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
        let valid = match name.strip_prefix('[').and_then(|n| n.strip_suffix(']')) {
            Some(ip) => ip.parse::<std::net::Ipv6Addr>().is_ok(),
            None => {
                !name.is_empty()
                    && (name.chars()).all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
            }
        };
        if !valid {
            return Err(format!("{s:?} isn't a host name or [IPv6] address"));
        }
        // Plain digits only: `parse` would also take `+80`.
        let port = match port {
            None => None,
            Some(p) if p.bytes().all(|b| b.is_ascii_digit()) && !p.starts_with('0') => {
                Some(p.parse().map_err(|_| format!("bad port {p:?}"))?)
            }
            Some(p) => return Err(format!("bad port {p:?}")),
        };
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
        let names = if addr.ip().is_loopback() {
            ["127.0.0.1", "localhost", "[::1]"]
                .map(String::from)
                .to_vec()
        } else {
            vec![match addr.ip() {
                IpAddr::V4(ip) => ip.to_string(),
                IpAddr::V6(ip) => format!("[{ip}]"),
            }]
        };
        let mut hosts: Vec<_> = (names.into_iter())
            .map(|name| HostName { name, port })
            .collect();
        hosts.extend(allowed.iter().map(|h| HostName {
            name: h.name.clone(),
            port: h.port.or(port),
        }));
        Guard { token, hosts }
    }

    /// Whether `host` is one of the daemon's names, taking it to be on
    /// `default_port` if it doesn't give one.
    fn allows(&self, host: &str, default_port: u16) -> bool {
        let Ok(mut host) = host.parse::<HostName>() else {
            return false;
        };
        host.port = host.port.or(Some(default_port));
        self.hosts.contains(&host)
    }

    /// Whether the request has exactly one `Host`, naming the daemon.
    fn host_allowed(&self, headers: &HeaderMap) -> bool {
        let mut hosts = headers.get_all(header::HOST).iter();
        match (hosts.next(), hosts.next()) {
            // A browser leaves out the port when it's HTTP's own.
            (Some(host), None) => host.to_str().is_ok_and(|h| self.allows(h, 80)),
            _ => false,
        }
    }

    /// Whether the request comes from one of the daemon's own pages, or
    /// from something that isn't a browser, which sends no `Origin`.
    fn origin_allowed(&self, headers: &HeaderMap) -> bool {
        headers.get_all(header::ORIGIN).iter().all(|origin| {
            let origin = origin.to_str().unwrap_or("");
            match origin.split_once("://") {
                Some(("http", host)) => self.allows(host, 80),
                Some(("https", host)) => self.allows(host, 443),
                _ => false,
            }
        })
    }

    fn is_token(&self, candidate: &str) -> bool {
        same(candidate.as_bytes(), self.token.as_bytes())
    }

    /// Whether `headers` carry the token, as a cookie or a bearer token.
    fn authorized(&self, headers: &HeaderMap) -> bool {
        let bearer = headers
            .get_all(header::AUTHORIZATION)
            .iter()
            .filter_map(|v| v.to_str().ok()?.trim().split_once(' '))
            .filter(|(scheme, _)| scheme.eq_ignore_ascii_case("bearer"))
            .map(|(_, token)| token);
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

/// The middleware: checks `Host`, then `Origin`, then the token. A page
/// loaded with the right `?token=` sets the cookie and sends the browser
/// back to the page without it, so the token doesn't stay in its address
/// bar or history.
pub(crate) async fn guard(State(guard): State<Arc<Guard>>, req: Request, next: Next) -> Response {
    let headers = req.headers();
    if !guard.host_allowed(headers) {
        return ApiError {
            status: StatusCode::MISDIRECTED_REQUEST,
            message: "this daemon isn't served under that name; see --allow-host".into(),
        }
        .into_response();
    }
    let reads = matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if !reads && !guard.origin_allowed(headers) {
        return ApiError {
            status: StatusCode::FORBIDDEN,
            message: "requests from other sites are refused".into(),
        }
        .into_response();
    }
    let authorized = guard.authorized(headers);
    let uri = req.uri();
    // Only the slash the path starts with, so the redirect can't be read as
    // `//other.site/`.
    let path = format!("/{}", uri.path().trim_start_matches(['/', '\\']));
    let page = matches!(*req.method(), Method::GET | Method::HEAD);
    if let (true, Some(query)) = (page, uri.query()) {
        let (tokens, rest): (Vec<_>, Vec<_>) = query
            .split('&')
            .partition(|pair| pair.split('=').next() == Some(PARAM));
        let valid = tokens
            .iter()
            .all(|t| t.split_once('=').is_some_and(|(_, v)| guard.is_token(v)));
        match (tokens.is_empty(), valid, authorized) {
            (true, ..) | (false, false, true) => {}
            (false, false, false) => return unauthorized(&path),
            (false, true, _) => {
                let cookie = format!(
                    "{COOKIE}={}; HttpOnly; SameSite=Strict; Path=/; Max-Age=31536000",
                    guard.token
                );
                let Ok(cookie) = HeaderValue::from_str(&cookie) else {
                    return ApiError {
                        status: StatusCode::INTERNAL_SERVER_ERROR,
                        message: "the token can't be sent as a cookie".into(),
                    }
                    .into_response();
                };
                let to = match rest.is_empty() {
                    true => path,
                    false => format!("{path}?{}", rest.join("&")),
                };
                let mut res = Redirect::to(&to).into_response();
                res.headers_mut().insert(header::SET_COOKIE, cookie);
                return res;
            }
        }
    }
    if !authorized {
        return unauthorized(&path);
    }
    next.run(req).await
}

/// The API's own error for its routes; for pages, one a person can read.
fn unauthorized(path: &str) -> Response {
    let rest = path.trim_start_matches('/');
    let api = rest.get(..3).is_some_and(|p| p.eq_ignore_ascii_case("api"))
        && matches!(rest.as_bytes().get(3), None | Some(b'/'));
    let mut res = if api {
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
        // A fully qualified name is a name of its own.
        assert_eq!(host("localhost.").unwrap().to_string(), "localhost.");
        for bad in [
            "",
            ":80",
            "a:b",
            "a:",
            "::1",
            "[::1",
            "[::1]80",
            "[::1]:",
            "[nope]",
            "a/b",
            "a b",
            "localhost:+80",
            "localhost:07878",
            "localhost:65536",
            "localhost:80:80",
        ] {
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
            let mode = |p: &Path| std::fs::metadata(p).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode(&path), 0o600);
            assert_eq!(mode(&dir.path().join("tb")), 0o700);
        }
        // Nothing is left behind but the token.
        let names: Vec<_> = std::fs::read_dir(dir.path().join("tb"))
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        assert_eq!(names, ["web-token"]);
        // Each place gets its own.
        let other = load_or_create_token(&dir.path().join("other")).unwrap();
        assert_ne!(other, token);
    }

    #[test]
    fn daemons_starting_together_share_one_token() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("tb/web-token");
        let tokens: Vec<String> = std::thread::scope(|s| {
            let started: Vec<_> = (0..8)
                .map(|_| s.spawn(|| load_or_create_token(&path).unwrap()))
                .collect();
            started.into_iter().map(|t| t.join().unwrap()).collect()
        });
        assert!(tokens.iter().all(|t| *t == tokens[0]), "{tokens:?}");
        assert_eq!(read_token(&path).unwrap().unwrap(), tokens[0]);
    }

    #[test]
    fn bad_token_files_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("web-token");
        for text in ["", "\n", "short", &"g".repeat(64), &"a".repeat(65)] {
            std::fs::write(&path, text).unwrap();
            let e = load_or_create_token(&path).unwrap_err();
            assert!(e.to_string().contains("delete it"), "{e}");
            // It's left for the user to look at.
            assert_eq!(std::fs::read_to_string(&path).unwrap(), text);
        }
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        let e = load_or_create_token(&path).unwrap_err();
        assert!(e.to_string().contains("isn't a regular file"), "{e}");
    }

    #[cfg(unix)]
    #[test]
    fn token_files_are_kept_private() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real");
        let token = load_or_create_token(&real).unwrap();

        // A symlink, even to a good token, isn't followed.
        let link = dir.path().join("link");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let e = load_or_create_token(&link).unwrap_err();
        assert!(e.to_string().contains("isn't a regular file"), "{e}");

        // One others can read is made private, and still used.
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert_eq!(load_or_create_token(&real).unwrap(), token);
        let mode = std::fs::metadata(&real).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
}
