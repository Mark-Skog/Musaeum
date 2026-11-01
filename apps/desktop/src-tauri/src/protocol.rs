//! The `tile` custom protocol, reached at `http://tile.localhost/<session>/<action>`.
//!
//! This is how the compositor worker talks to the host without going through the main
//! thread or JSON:
//!
//! | Request | Answer |
//! |---------|--------|
//! | `GET /<session>/geometry` | Page-size table |
//! | `GET /<session>/clock` | Host clock as a little-endian `u64`, in microseconds |
//! | `POST /<session>/demand` | 204; the body is a binary demand list |
//! | `GET /<session>/next` | Path A: waits, then returns a batch of tiles; 204 if closing |
//!
//! The handler runs on the UI thread, so it only parses the path and hands work over.

use std::borrow::Cow;

use tauri::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use tauri::{AppHandle, UriSchemeResponder};

use crate::session::app_state;

/// The protocol's scheme name.
pub const SCHEME: &str = "tile";

/// Only the app's own pages may use this protocol. Book content is served from other origins,
/// and must never be able to reach the renderer from there.
pub fn is_allowed_origin(origin: &str) -> bool {
    matches!(
        origin,
        "http://tauri.localhost"
            | "https://tauri.localhost"
            | "tauri://localhost"
            | "http://localhost:1420"
    )
}

fn respond(responder: UriSchemeResponder, origin: &HeaderValue, status: StatusCode, body: Vec<u8>) {
    let response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, "application/octet-stream")
        .header(header::CACHE_CONTROL, "no-store")
        .header(header::VARY, "Origin")
        .header(header::ACCESS_CONTROL_ALLOW_ORIGIN, origin)
        .header(header::ACCESS_CONTROL_ALLOW_METHODS, "GET, POST, OPTIONS")
        .header(header::ACCESS_CONTROL_ALLOW_HEADERS, "*")
        // The page is cross-origin isolated, so every response must opt in explicitly.
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .body(Cow::<'static, [u8]>::Owned(body));
    match response {
        Ok(response) => responder.respond(response),
        Err(error) => {
            eprintln!("could not build a tile response: {error}");
            respond_status_only(responder, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}

fn respond_status_only(responder: UriSchemeResponder, status: StatusCode) {
    let mut response = Response::new(Cow::<'static, [u8]>::Borrowed(&[]));
    *response.status_mut() = status;
    responder.respond(response);
}

/// Handles one request.
pub fn handle(app: &AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    let Some(origin) = request
        .headers()
        .get(header::ORIGIN)
        .filter(|origin| origin.to_str().is_ok_and(is_allowed_origin))
        .cloned()
    else {
        respond_status_only(responder, StatusCode::FORBIDDEN);
        return;
    };

    if request.method() == Method::OPTIONS {
        respond(responder, &origin, StatusCode::NO_CONTENT, Vec::new());
        return;
    }

    let Some((session_id, action)) = parse_path(request.uri().path()) else {
        respond(responder, &origin, StatusCode::NOT_FOUND, Vec::new());
        return;
    };
    let Some(session) = app_state(app).session(session_id) else {
        respond(responder, &origin, StatusCode::GONE, Vec::new());
        return;
    };

    match (request.method().clone(), action) {
        (Method::GET, "geometry") => {
            respond(responder, &origin, StatusCode::OK, session.geometry.clone());
        }
        (Method::GET, "clock") => {
            let now = session.pool.clock_us().to_le_bytes().to_vec();
            respond(responder, &origin, StatusCode::OK, now);
        }
        (Method::POST, "demand") => {
            session.pool.submit_demand(request.into_body());
            respond(responder, &origin, StatusCode::NO_CONTENT, Vec::new());
        }
        (Method::GET, "next") => {
            session.pool.park_poller(Box::new(move |batch| {
                let status = if batch.is_empty() {
                    StatusCode::NO_CONTENT
                } else {
                    StatusCode::OK
                };
                respond(responder, &origin, status, batch);
            }));
        }
        _ => respond(responder, &origin, StatusCode::NOT_FOUND, Vec::new()),
    }
}

fn parse_path(path: &str) -> Option<(u32, &str)> {
    let (session, action) = path.strip_prefix('/')?.split_once('/')?;
    Some((session.parse().ok()?, action))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_need_a_numeric_session_and_an_action() {
        assert_eq!(parse_path("/3/next"), Some((3, "next")));
        assert_eq!(parse_path("/3"), None);
        assert_eq!(parse_path("/x/next"), None);
        assert_eq!(parse_path("3/next"), None);
        assert_eq!(parse_path("/"), None);
    }

    #[test]
    fn only_the_apps_own_origins_are_allowed() {
        assert!(is_allowed_origin("http://tauri.localhost"));
        assert!(is_allowed_origin("http://localhost:1420"));
        assert!(!is_allowed_origin("http://evil.example"));
        assert!(!is_allowed_origin("http://book.localhost"));
        assert!(!is_allowed_origin("http://book.0123456789abcdef.localhost"));
        assert!(!is_allowed_origin("null"));
    }
}
