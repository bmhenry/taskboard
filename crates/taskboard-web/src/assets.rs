//! The web UI's files when they're built into the daemon: trunk's `dist/`,
//! embedded with `rust-embed` by the `embed` feature (which
//! `scripts/build-web.sh` turns on).
//!
//! `index.html` is revalidated on every load, so a new daemon's UI shows up
//! straight away; the scripts and wasm it loads have trunk's content hash in
//! their names, so browsers keep them for good. Each file has an `ETag`, and
//! a `.br` or `.gz` copy of it next to it (the build script makes them) is
//! sent instead to browsers that take one.

use axum::http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use rust_embed::EmbeddedFile;

/// A set of embedded files: a `RustEmbed`'s `get`.
pub type Bundle = fn(&str) -> Option<EmbeddedFile>;

/// The web UI built into this binary, if it was built with one.
#[cfg(feature = "embed")]
pub const BUILT_IN: Option<Bundle> = Some(<Dist as rust_embed::RustEmbed>::get);
#[cfg(not(feature = "embed"))]
pub const BUILT_IN: Option<Bundle> = None;

/// trunk's output, which `build.rs` checks is there.
#[cfg(feature = "embed")]
#[derive(rust_embed::RustEmbed)]
#[folder = "../taskboard-wasm/dist"]
struct Dist;

/// How long browsers keep files whose names change with their contents.
pub(crate) const FOREVER: &str = "private, max-age=31536000, immutable";

/// Compressed copies looked for, best first: the encoding and its suffix.
const ENCODINGS: [(&str, &str); 2] = [("br", ".br"), ("gzip", ".gz")];

/// Serves `uri`'s file from `bundle`, `/` being `index.html`.
pub fn serve(bundle: Bundle, method: &Method, uri: &Uri, headers: &HeaderMap) -> Response {
    if method != Method::GET && method != Method::HEAD {
        return StatusCode::METHOD_NOT_ALLOWED.into_response();
    }
    let path = match uri.path().trim_start_matches('/') {
        "" => "index.html",
        path => path,
    };
    let Some(file) = bundle(path) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut content_type = file.metadata.mimetype().to_owned();
    if content_type.starts_with("text/") {
        content_type.push_str("; charset=utf-8");
    }
    let accepted = ENCODINGS.iter().find_map(|&(encoding, suffix)| {
        let copy = bundle(&format!("{path}{suffix}"))?;
        accepts(headers, encoding).then_some((encoding, copy))
    });
    let (encoding, file) = match accepted {
        Some((encoding, copy)) => (Some(encoding), copy),
        None => (None, file),
    };
    // The copy's own hash, so each encoding has its own tag.
    let etag = format!("\"{}\"", hex(&file.metadata.sha256_hash()[..16]));

    let mut res = if if_none_match(headers, &etag) {
        StatusCode::NOT_MODIFIED.into_response()
    } else {
        let mut res = file.data.into_owned().into_response();
        let headers = res.headers_mut();
        headers.insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(&content_type).expect("MIME types are plain ASCII"),
        );
        if let Some(encoding) = encoding {
            headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static(encoding));
        }
        res
    };
    let headers = res.headers_mut();
    let cache = if is_hashed(path) { FOREVER } else { "no-cache" };
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    headers.insert(
        header::ETAG,
        HeaderValue::from_str(&etag).expect("hex is plain ASCII"),
    );
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    res
}

/// Whether `Accept-Encoding` takes `encoding`, with a nonzero weight.
fn accepts(headers: &HeaderMap, encoding: &str) -> bool {
    let values = headers.get_all(header::ACCEPT_ENCODING).iter();
    values
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .any(|item| {
            let mut parts = item.split(';').map(str::trim);
            let name = parts.next().unwrap_or("");
            let weight = parts
                .find_map(|p| p.strip_prefix("q=").or_else(|| p.strip_prefix("Q=")))
                .and_then(|q| q.parse::<f32>().ok())
                .unwrap_or(1.0);
            (name.eq_ignore_ascii_case(encoding) || name == "*") && weight > 0.0
        })
}

/// Whether `If-None-Match` names `etag` (or anything, with `*`).
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    let values = headers.get_all(header::IF_NONE_MATCH).iter();
    values
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|tag| tag.trim().trim_start_matches("W/"))
        .any(|tag| tag == etag || tag == "*")
}

/// Whether `path` has a content hash in it, as trunk names the files
/// `index.html` loads (`taskboard-wasm-<16 hex digits>_bg.wasm`, and
/// wasm-bindgen's `snippets/<crate>-<hash>/`).
fn is_hashed(path: &str) -> bool {
    path.split('/').any(|segment| {
        segment
            .rsplit_once('-')
            .is_some_and(|(_, tail)| tail.bytes().take_while(u8::is_ascii_hexdigit).count() >= 16)
    })
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hashed_names_are_trunks() {
        for path in [
            "taskboard-wasm-1a2b3c4d5e6f7a8b.js",
            "taskboard-wasm-1a2b3c4d5e6f7a8b_bg.wasm",
            "taskboard-wasm-1a2b3c4d5e6f7a8b_bg.wasm.gz",
            "snippets/iced-0123456789abcdef0/inline0.js",
        ] {
            assert!(is_hashed(path), "{path}");
        }
        for path in [
            "index.html",
            "taskboard-wasm.js",
            "a-cafe.js",
            "favicon.ico",
        ] {
            assert!(!is_hashed(path), "{path}");
        }
    }

    #[test]
    fn encodings_with_no_weight_are_refused() {
        let with = |value: &'static str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_static(value));
            headers
        };
        assert!(accepts(&with("gzip, deflate, br, zstd"), "br"));
        assert!(accepts(&with("GZIP;q=0.5"), "gzip"));
        assert!(accepts(&with("*"), "gzip"));
        assert!(!accepts(&with("gzip;q=0, br"), "gzip"));
        assert!(!accepts(&with("identity"), "gzip"));
        assert!(!accepts(&HeaderMap::new(), "gzip"));
    }
}
