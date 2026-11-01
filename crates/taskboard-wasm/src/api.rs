//! Requests to the daemon's API (`taskboard_core::api`), with the browser's
//! `fetch`. The page is served by the daemon, so every request is to the same
//! origin and carries its sign-in cookie.

use js_sys::Uint8Array;
use serde::Serialize;
use serde::de::DeserializeOwned;
use taskboard_core::api::ErrorBody;
use wasm_bindgen::{JsCast, JsValue};
use wasm_bindgen_futures::JsFuture;
use web_sys::{Headers, Request, RequestCredentials, RequestInit, Response};

/// What a request the daemon refused for want of its sign-in cookie fails
/// with.
pub const UNAUTHORIZED: &str = "Not signed in to the daemon: open the link it printed when it started (the one ending in ?token=…), then reload this page";

/// What's sent with a request.
pub enum Body {
    None,
    Json(String),
    /// A file's raw contents, for an upload.
    Bytes(Vec<u8>),
}

impl Body {
    pub fn json(value: &impl Serialize) -> Result<Body, String> {
        serde_json::to_string(value)
            .map(Body::Json)
            .map_err(|e| e.to_string())
    }
}

/// Sends a request to `url` and returns the response, if it succeeded. A
/// failure is the daemon's [`ErrorBody`], or says why there wasn't one.
pub async fn send(method: &str, url: &str, body: Body) -> Result<Response, String> {
    let init = RequestInit::new();
    init.set_method(method);
    init.set_credentials(RequestCredentials::SameOrigin);
    let content = match body {
        Body::None => None,
        Body::Json(text) => Some(("application/json", JsValue::from_str(&text))),
        Body::Bytes(bytes) => Some((
            "application/octet-stream",
            Uint8Array::from(bytes.as_slice()).into(),
        )),
    };
    if let Some((kind, body)) = content {
        let headers = Headers::new().map_err(js_error)?;
        headers.set("Content-Type", kind).map_err(js_error)?;
        init.set_headers(&headers);
        init.set_body(&body);
    }
    let request = Request::new_with_str_and_init(url, &init).map_err(js_error)?;
    let window = web_sys::window().ok_or("no browser window")?;
    let res: Response = JsFuture::from(window.fetch_with_request(&request))
        .await
        .map_err(|e| format!("couldn't reach the daemon: {}", js_error(e)))?
        .unchecked_into();
    if res.ok() {
        return Ok(res);
    }
    if res.status() == 401 {
        return Err(UNAUTHORIZED.into());
    }
    let status = format!("{} {}", res.status(), res.status_text());
    let text = text(&res).await.unwrap_or_default();
    Err(serde_json::from_str::<ErrorBody>(&text)
        .map(|b| b.error)
        .unwrap_or_else(|_| format!("the daemon answered {status}")))
}

/// [`send`], reading the response as JSON.
pub async fn json<T: DeserializeOwned>(method: &str, url: &str, body: Body) -> Result<T, String> {
    let res = send(method, url, body).await?;
    let text = text(&res).await?;
    serde_json::from_str(&text).map_err(|e| format!("unexpected answer from the daemon: {e}"))
}

/// [`send`], returning the response's bytes.
pub async fn bytes(url: &str) -> Result<Vec<u8>, String> {
    let res = send("GET", url, Body::None).await?;
    let buffer = JsFuture::from(res.array_buffer().map_err(js_error)?)
        .await
        .map_err(js_error)?;
    Ok(Uint8Array::new(&buffer).to_vec())
}

async fn text(res: &Response) -> Result<String, String> {
    let text = JsFuture::from(res.text().map_err(js_error)?)
        .await
        .map_err(js_error)?;
    Ok(text.as_string().unwrap_or_default())
}

/// `value` from JavaScript, as a message: an `Error`'s, or the value itself.
pub fn js_error(value: JsValue) -> String {
    if let Some(e) = value.dyn_ref::<js_sys::Error>() {
        return String::from(e.message());
    }
    value.as_string().unwrap_or_else(|| format!("{value:?}"))
}

/// `text` encoded for a URL's query.
pub fn encode(text: &str) -> String {
    String::from(js_sys::encode_uri_component(text))
}
