//! The `book` custom protocol: where the fluid reader lives, at an origin of its own.
//!
//! Each open book gets `http://book.<random>.localhost/`, separate from the app and from any
//! other book. Tauri treats its registered protocols' own hosts as the app's "local" origins, so
//! it matters that this one is not `book.localhost`: anything served here counts as remote, and
//! the capabilities in `capabilities/default.json` give remote content nothing.
//!
//! | Request | Answer |
//! |---------|--------|
//! | `GET /reader.html` | The reader's page, with a nonce that only its own scripts have |
//! | `GET /assets/...` | The reader's scripts |
//! | `GET /book.epub` | The book's bytes, for a book opened from the command line |
//! | `GET /pkg/...` | A file of the normalized book, from the cache, for a book of the library |
//!
//! The policy below is what stands between a book's scripts and the world, along with the
//! `sandbox` attribute of the iframes. foliate-js turns each section into a `blob:` document,
//! and a `blob:` document inherits the policy of the page that made it, so one header covers
//! every section of the book.

use std::borrow::Cow;
use std::hash::{BuildHasher, Hasher, RandomState};
use std::path::{Path, PathBuf};
use std::sync::{Arc, PoisonError, RwLock};
use std::thread;

use serde::Serialize;
use tauri::http::{HeaderValue, Method, Request, Response, StatusCode, header};
use tauri::{AppHandle, Manager, State, UriSchemeResponder};

use crate::flow::FlowBook;
use crate::options::BookProtection;
use crate::protocol::is_allowed_origin;
use crate::session::AppState;

/// The protocol's scheme name.
pub const SCHEME: &str = "book";

/// The `sandbox` attribute of a section's iframe, when scripts stay out of it.
const SECTION_SANDBOX: &str = "allow-same-origin";
/// The same, with what the WebKit bug in foliate-js forces on that engine. The experiment uses
/// it to show that the policy holds without the sandbox.
const SECTION_SANDBOX_WITH_SCRIPTS: &str = "allow-same-origin allow-scripts";

/// What an open book is made of.
enum BookContent {
    /// An EPUB file as it is, from the command line: the benchmarks of Phases 0 and 3.
    Epub(PathBuf),
    /// A book of the library, normalized, served file by file from the cache.
    Package(Arc<FlowBook>),
}

/// An open book.
pub struct BookSession {
    /// The random part of this book's origin.
    id: String,
    content: BookContent,
    protection: BookProtection,
}

/// Holds the open book, for the protocol handler to find.
#[derive(Default)]
pub struct BookState {
    current: RwLock<Option<Arc<BookSession>>>,
}

/// 128 unpredictable bits, in hex. `RandomState` is keyed from the operating system's random
/// source, and each new one hashes differently.
fn random_token() -> String {
    let mut token = String::with_capacity(32);
    for _ in 0..2 {
        let hasher = RandomState::new().build_hasher();
        token.push_str(&format!("{:016x}", hasher.finish()));
    }
    token
}

/// The Content Security Policy of the reader and of every section of the book.
///
/// `script-src` names a nonce and nothing else, on purpose. A `'self'` here would let a section
/// load the app's own scripts, which share this origin. The nonce travels with the reader's
/// module graph, and a book has no way to learn it: reading it takes a script, and browsers
/// hide the attribute from CSS.
fn content_security_policy(nonce: &str) -> String {
    policy_with(&format!("script-src 'nonce-{nonce}'"))
}

/// The policy, with `script` as its `script-src`.
fn policy_with(script: &str) -> String {
    [
        "default-src 'none'".to_owned(),
        script.to_owned(),
        // foliate-js injects <style> elements and books use style attributes.
        "style-src 'unsafe-inline' blob:".to_owned(),
        "img-src blob: data:".to_owned(),
        "font-src blob: data:".to_owned(),
        "media-src blob:".to_owned(),
        // Only the reader's fetch of the book. Sections cannot run anything to use this.
        "connect-src 'self'".to_owned(),
        // Sections are `blob:` documents. This also stops a section from being navigated
        // elsewhere, because the check applies to every navigation of the frame.
        "frame-src blob:".to_owned(),
        "worker-src 'none'".to_owned(),
        "object-src 'none'".to_owned(),
        "base-uri 'none'".to_owned(),
        "form-action 'none'".to_owned(),
        "manifest-src 'none'".to_owned(),
        "frame-ancestors http://tauri.localhost https://tauri.localhost tauri://localhost \
         http://localhost:1420"
            .to_owned(),
    ]
    .join("; ")
}

/// Tauri adds a nonce of its own to the scripts it embeds. This replaces it with `nonce`, on the
/// script tags of the reader's page and on the links that preload its modules, and adds it where
/// there was none. A preload without the nonce is refused by the policy, and its module is then
/// fetched a second time, by the import.
fn with_script_nonce(html: &str, nonce: &str) -> String {
    let mut out = String::with_capacity(html.len() + 128);
    let mut rest = html;
    loop {
        let (at, name) = match (rest.find("<script"), rest.find("<link")) {
            (Some(script), Some(link)) if link < script => (link, "<link"),
            (Some(script), _) => (script, "<script"),
            (None, Some(link)) => (link, "<link"),
            (None, None) => break,
        };
        let (before, tag_and_rest) = rest.split_at(at);
        out.push_str(before);
        let end = tag_and_rest
            .find('>')
            .map_or(tag_and_rest.len(), |at| at + 1);
        let (tag, after) = tag_and_rest.split_at(end);
        let attributes = &tag[name.len()..];
        if name == "<script" || is_module_preload(attributes) {
            out.push_str(name);
            out.push_str(" nonce=\"");
            out.push_str(nonce);
            out.push('"');
            out.push_str(&strip_nonce_attribute(attributes));
        } else {
            out.push_str(tag);
        }
        rest = after;
    }
    out.push_str(rest);
    out
}

fn is_module_preload(attributes: &str) -> bool {
    attributes.contains("rel=\"modulepreload\"") || attributes.contains("rel=modulepreload")
}

fn strip_nonce_attribute(attributes: &str) -> String {
    let Some(start) = attributes.find(" nonce=\"") else {
        return attributes.to_owned();
    };
    let value_at = start + " nonce=\"".len();
    let Some(length) = attributes[value_at..].find('"') else {
        return attributes.to_owned();
    };
    format!(
        "{}{}",
        &attributes[..start],
        &attributes[value_at + length + 1..]
    )
}

/// The same policy for the files of a normalized book, which have no script of the reader's and
/// may never run one: nothing can carry a nonce.
fn package_policy() -> String {
    policy_with("script-src 'none'")
}

/// The media type of a file of a normalized book, by its name, which the normalizer chose.
fn content_type_of(path: &str) -> &'static str {
    let extension = path
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "xhtml" | "xht" | "html" | "htm" => "application/xhtml+xml",
        "opf" => "application/oebps-package+xml",
        "xml" | "ncx" => "application/xml",
        "json" => "application/json",
        "css" => "text/css",
        "svg" => "image/svg+xml",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "avif" => "image/avif",
        "bmp" => "image/bmp",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "otf" => "font/otf",
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" => "video/mp4",
        "ogg" | "oga" => "audio/ogg",
        "webm" => "video/webm",
        _ => "application/octet-stream",
    }
}

/// Shows a book of the library in the reader, at a new origin. `host_origin` is the page asking,
/// the only one the reader will talk to.
pub fn open_package(
    books: &BookState,
    host_origin: &str,
    flow: Arc<FlowBook>,
    e2e: bool,
) -> Result<(String, String), String> {
    if !is_allowed_origin(host_origin) {
        return Err(format!("{host_origin} cannot open a book"));
    }
    let session = Arc::new(BookSession {
        id: random_token(),
        content: BookContent::Package(flow),
        protection: BookProtection::Full,
    });
    let origin = format!("http://book.{}.localhost", session.id);
    let reader_url = format!(
        "{origin}/reader.html?host={host_origin}&sections={}&package=1{}",
        SECTION_SANDBOX.replace(' ', "%20"),
        if e2e { "&e2e=1" } else { "" }
    );
    *books
        .current
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(session);
    Ok((origin, reader_url))
}

/// The normalized book open in the reader, if it is this document.
pub fn open_flow_book(books: &BookState, document: i64) -> Option<Arc<FlowBook>> {
    let current = books
        .current
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone()?;
    match &current.content {
        BookContent::Package(flow) if flow.document_id == document => Some(Arc::clone(flow)),
        _ => None,
    }
}

/// Closes the book in the reader. Its origin answers nothing from now on.
pub fn close(books: &BookState) {
    books
        .current
        .write()
        .unwrap_or_else(PoisonError::into_inner)
        .take();
}

/// What the frontend needs to create the reader's iframe.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BookInfo {
    origin: String,
    reader_url: String,
    size_bytes: u64,
    protection: BookProtection,
}

/// Opens the book named on the command line. `host_origin` is the page asking, which is the only
/// one the reader will talk to.
#[tauri::command(async)]
pub fn open_book(
    state: State<'_, AppState>,
    books: State<'_, BookState>,
    host_origin: String,
) -> Result<BookInfo, String> {
    if !is_allowed_origin(&host_origin) {
        return Err(format!("{host_origin} cannot open a book"));
    }
    let options = state.options();
    let path = options.epub.clone().ok_or("no book to open")?;
    let size_bytes = std::fs::metadata(&path)
        .map_err(|error| format!("opening {}: {error}", path.display()))?
        .len();

    let session = Arc::new(BookSession {
        id: random_token(),
        content: BookContent::Epub(path),
        protection: options.book_protection,
    });
    let origin = format!("http://book.{}.localhost", session.id);
    let sections = match session.protection {
        BookProtection::Full | BookProtection::SandboxOnly => SECTION_SANDBOX,
        BookProtection::CspOnly | BookProtection::None => SECTION_SANDBOX_WITH_SCRIPTS,
    };
    let reader_url = format!(
        "{origin}/reader.html?host={host_origin}&sections={}",
        sections.replace(' ', "%20")
    );
    let protection = session.protection;
    *books
        .current
        .write()
        .unwrap_or_else(PoisonError::into_inner) = Some(session);
    Ok(BookInfo {
        origin,
        reader_url,
        size_bytes,
        protection,
    })
}

fn respond(responder: UriSchemeResponder, status: StatusCode) {
    let mut response = Response::new(Cow::<'static, [u8]>::Borrowed(&[]));
    *response.status_mut() = status;
    responder.respond(response);
}

fn respond_with(
    responder: UriSchemeResponder,
    content_type: &str,
    policy: Option<&str>,
    body: Vec<u8>,
) {
    let mut response = Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CACHE_CONTROL, "no-store")
        // The main page is cross-origin isolated, so whatever it embeds must opt in.
        .header("Cross-Origin-Resource-Policy", "cross-origin")
        .header("Cross-Origin-Embedder-Policy", "require-corp")
        .header("X-Content-Type-Options", "nosniff");
    if let Some(policy) = policy {
        response = response.header(header::CONTENT_SECURITY_POLICY, policy);
    }
    match response.body(Cow::<'static, [u8]>::Owned(body)) {
        Ok(response) => responder.respond(response),
        Err(error) => {
            eprintln!("could not build a book response: {error}");
            respond(responder, StatusCode::INTERNAL_SERVER_ERROR);
        }
    }
}

/// Handles one request. Runs on the UI thread, so reading the book happens elsewhere.
pub fn handle(app: &AppHandle, request: Request<Vec<u8>>, responder: UriSchemeResponder) {
    if request.method() != Method::GET {
        respond(responder, StatusCode::METHOD_NOT_ALLOWED);
        return;
    }
    let current = app
        .state::<BookState>()
        .current
        .read()
        .unwrap_or_else(PoisonError::into_inner)
        .clone();
    let Some(session) = current.filter(|session| belongs_to(&request, &session.id)) else {
        respond(responder, StatusCode::GONE);
        return;
    };

    // A request from somewhere else in the app, or from another book, has no business here.
    let own_origin = format!("http://book.{}.localhost", session.id);
    let foreign = request
        .headers()
        .get(header::ORIGIN)
        .is_some_and(|origin: &HeaderValue| origin.as_bytes() != own_origin.as_bytes());
    if foreign {
        respond(responder, StatusCode::FORBIDDEN);
        return;
    }

    match (request.uri().path(), &session.content) {
        ("/book.epub", BookContent::Epub(path)) => serve_book(path.clone(), responder),
        ("/reader.html", _) => serve_reader_page(app, session.protection, responder),
        (path, _) if path.starts_with("/assets/") => serve_script(app, path, responder),
        (path, BookContent::Package(flow)) if path.starts_with("/pkg/") => {
            serve_package_file(Arc::clone(flow), &path["/pkg/".len()..], responder);
        }
        _ => respond(responder, StatusCode::NOT_FOUND),
    }
}

/// On Windows the host arrives as `<id>.localhost`, with the scheme's name already taken off.
fn belongs_to(request: &Request<Vec<u8>>, id: &str) -> bool {
    request
        .uri()
        .host()
        .and_then(|host| host.strip_suffix(".localhost"))
        .is_some_and(|host| host == id || host.strip_prefix("book.") == Some(id))
}

fn serve_book(path: PathBuf, responder: UriSchemeResponder) {
    thread::spawn(move || match read_book(&path) {
        Ok(bytes) => respond_with(responder, "application/epub+zip", None, bytes),
        Err(error) => {
            eprintln!("reading {}: {error}", path.display());
            respond(responder, StatusCode::NOT_FOUND);
        }
    });
}

fn read_book(path: &Path) -> std::io::Result<Vec<u8>> {
    std::fs::read(path)
}

/// A file of a normalized book. The path is checked the way the cache checked it when it was
/// written: relative, a few plain segments, nothing that climbs out.
fn serve_package_file(flow: Arc<FlowBook>, path: &str, responder: UriSchemeResponder) {
    let Ok(path) = percent_decoded(path) else {
        respond(responder, StatusCode::NOT_FOUND);
        return;
    };
    if !musaeum_ingest::protocol::is_safe_path(&path) {
        respond(responder, StatusCode::NOT_FOUND);
        return;
    }
    thread::spawn(move || match std::fs::read(flow.dir.join(&path)) {
        Ok(bytes) => respond_with(
            responder,
            content_type_of(&path),
            Some(&package_policy()),
            bytes,
        ),
        Err(_) => respond(responder, StatusCode::NOT_FOUND),
    });
}

/// `%XX` in a request's path, as UTF-8. Anything malformed is refused rather than guessed at.
fn percent_decoded(path: &str) -> Result<String, ()> {
    let bytes = path.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        if bytes[at] == b'%' {
            let hex = path.get(at + 1..at + 3).ok_or(())?;
            out.push(u8::from_str_radix(hex, 16).map_err(|_| ())?);
            at += 3;
        } else {
            out.push(bytes[at]);
            at += 1;
        }
    }
    String::from_utf8(out).map_err(|_| ())
}

fn serve_reader_page(app: &AppHandle, protection: BookProtection, responder: UriSchemeResponder) {
    let Some(asset) = app.asset_resolver().get("reader.html".to_owned()) else {
        respond(responder, StatusCode::NOT_FOUND);
        return;
    };
    let Ok(html) = std::str::from_utf8(&asset.bytes) else {
        respond(responder, StatusCode::INTERNAL_SERVER_ERROR);
        return;
    };
    let nonce = random_token();
    let policy = content_security_policy(&nonce);
    let enforced = matches!(protection, BookProtection::Full | BookProtection::CspOnly);
    respond_with(
        responder,
        "text/html; charset=utf-8",
        enforced.then_some(policy.as_str()),
        with_script_nonce(html, &nonce).into_bytes(),
    );
}

fn serve_script(app: &AppHandle, path: &str, responder: UriSchemeResponder) {
    // The resolver answers anything unknown with the app's own page, which must not leak here.
    let asset = app
        .asset_resolver()
        .get(path.to_owned())
        .filter(|asset| asset.mime_type.contains("javascript"));
    match asset {
        Some(asset) => respond_with(responder, "text/javascript", None, asset.bytes.clone()),
        None => respond(responder, StatusCode::NOT_FOUND),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_policy_leaves_no_way_to_run_or_send_anything() {
        let policy = content_security_policy("abc");
        let directive = |name: &str| {
            policy
                .split("; ")
                .find(|directive| directive.starts_with(name))
                .unwrap_or_else(|| panic!("{name} is missing"))
                .to_owned()
        };
        assert_eq!(directive("default-src"), "default-src 'none'");
        assert_eq!(directive("script-src"), "script-src 'nonce-abc'");
        assert_eq!(directive("object-src"), "object-src 'none'");
        assert_eq!(directive("base-uri"), "base-uri 'none'");
        assert_eq!(directive("form-action"), "form-action 'none'");
        assert_eq!(directive("worker-src"), "worker-src 'none'");
        assert_eq!(directive("connect-src"), "connect-src 'self'");
        assert_eq!(directive("frame-src"), "frame-src blob:");
        for directive in policy.split("; ") {
            assert!(!directive.contains("http://127"), "{directive}");
            assert!(!directive.contains('*'), "a wildcard in {directive}");
            assert!(!directive.contains("unsafe-eval"), "{directive}");
        }
        assert!(!directive("script-src").contains("unsafe-inline"));
        assert!(!directive("script-src").contains("'self'"));
    }

    #[test]
    fn the_files_of_a_book_can_never_run_a_script() {
        let policy = package_policy();
        assert!(
            policy
                .split("; ")
                .any(|directive| directive == "script-src 'none'")
        );
        assert!(!policy.contains("script-src 'nonce-"));
        assert!(policy.contains("default-src 'none'"));
        assert!(policy.contains("connect-src 'self'"));
    }

    #[test]
    fn package_paths_are_decoded_strictly() {
        assert_eq!(
            percent_decoded("text/0001.xhtml"),
            Ok("text/0001.xhtml".to_owned())
        );
        assert_eq!(
            percent_decoded("images/a%20b.png"),
            Ok("images/a b.png".to_owned())
        );
        assert_eq!(percent_decoded("a%2"), Err(()));
        assert_eq!(percent_decoded("a%zz"), Err(()));
        assert_eq!(percent_decoded("%ff"), Err(()));
        assert!(!musaeum_ingest::protocol::is_safe_path("../library.db"));
        assert!(!musaeum_ingest::protocol::is_safe_path("text/../../x"));
        assert_eq!(content_type_of("text/0001.xhtml"), "application/xhtml+xml");
        assert_eq!(content_type_of("styles/a.CSS"), "text/css");
        assert_eq!(content_type_of("noextension"), "application/octet-stream");
    }

    #[test]
    fn tokens_are_long_and_do_not_repeat() {
        let (a, b) = (random_token(), random_token());
        assert_eq!(a.len(), 32);
        assert_ne!(a, b);
        assert!(a.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn every_script_tag_gets_the_nonce() {
        let html = r#"<head><script type="module" src="/a.js"></script></head><script>1</script>"#;
        assert_eq!(
            with_script_nonce(html, "n"),
            r#"<head><script nonce="n" type="module" src="/a.js"></script></head><script nonce="n">1</script>"#
        );
    }

    #[test]
    fn a_nonce_tauri_added_is_replaced() {
        let html = r#"<script nonce="tauri" type="module" src="/a.js"></script>"#;
        assert_eq!(
            with_script_nonce(html, "n"),
            r#"<script nonce="n" type="module" src="/a.js"></script>"#
        );
    }

    #[test]
    fn a_page_without_scripts_is_left_alone() {
        assert_eq!(with_script_nonce("<p>hi</p>", "n"), "<p>hi</p>");
    }

    #[test]
    fn module_preloads_get_the_nonce_and_other_links_do_not() {
        let html = concat!(
            r#"<link rel="stylesheet" href="/s.css">"#,
            r#"<script type="module" crossorigin src="/assets/reader.js"></script>"#,
            r#"<link rel="modulepreload" crossorigin href="/assets/metrics.js">"#,
            r#"<link nonce="tauri" rel="modulepreload" href="/assets/other.js">"#,
        );
        assert_eq!(
            with_script_nonce(html, "n"),
            concat!(
                r#"<link rel="stylesheet" href="/s.css">"#,
                r#"<script nonce="n" type="module" crossorigin src="/assets/reader.js"></script>"#,
                r#"<link nonce="n" rel="modulepreload" crossorigin href="/assets/metrics.js">"#,
                r#"<link nonce="n" rel="modulepreload" href="/assets/other.js">"#,
            )
        );
    }
}
