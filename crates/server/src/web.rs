//! Serves the embedded Dioxus UI (single-page app) and the PWA files.

use axum::body::Body;
use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;

/// The compiled Dioxus web app (`scripts/build.sh` puts it there).
#[derive(RustEmbed)]
#[folder = "../../dist/public"]
struct Ui;

/// PWA manifest and icons.
#[derive(RustEmbed)]
#[folder = "static"]
struct Static;

const HEAD_EXTRA: &str = r##"<link rel="manifest" href="/manifest.webmanifest">
<meta name="theme-color" content="#7c3aed">
<meta name="viewport" content="width=device-width, initial-scale=1">
<link rel="icon" href="/icon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/icon-192.png">
</head>"##;

const NOT_BUILT: &str = r#"<!doctype html><meta charset="utf-8"><title>Musik Faktory</title>
<body style="font-family:system-ui;max-width:40rem;margin:4rem auto;padding:0 1rem">
<h1>Musik Faktory server is running</h1>
<p>The user interface was not built into this binary. Build it with:</p>
<pre>scripts/build.sh</pre>
<p>The API is available under <code>/api/</code>.</p>"#;

fn file_response(path: &str, data: std::borrow::Cow<'static, [u8]>, mime: &str) -> Response {
    let cache = if path.contains("assets/") || path.contains("wasm/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    Response::builder()
        .header(header::CONTENT_TYPE, mime)
        .header(header::CACHE_CONTROL, cache)
        .body(Body::from(data.into_owned()))
        .unwrap()
}

fn index() -> Response {
    match Ui::get("index.html") {
        Some(f) => {
            let html = String::from_utf8_lossy(&f.data).replacen("</head>", HEAD_EXTRA, 1);
            Response::builder()
                .header(header::CONTENT_TYPE, "text/html; charset=utf-8")
                .header(header::CACHE_CONTROL, "no-cache")
                .body(Body::from(html))
                .unwrap()
        }
        None => (StatusCode::OK, [(header::CONTENT_TYPE, "text/html; charset=utf-8")], NOT_BUILT).into_response(),
    }
}

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    if path.starts_with("api/") {
        return (StatusCode::NOT_FOUND, axum::Json(musik_shared::ApiError { error: "Unknown API endpoint.".into(), ..Default::default() }))
            .into_response();
    }
    if path.is_empty() || path == "index.html" {
        return index();
    }
    if let Some(f) = Static::get(path) {
        let mime = if path.ends_with(".webmanifest") { "application/manifest+json".to_string() } else { f.metadata.mimetype().to_string() };
        return file_response(path, f.data, &mime);
    }
    if let Some(f) = Ui::get(path) {
        let mime = f.metadata.mimetype().to_string();
        return file_response(path, f.data, &mime);
    }
    // Looks like a file but doesn't exist: real 404. Otherwise it's a client-side route.
    let last = path.rsplit('/').next().unwrap_or("");
    if last.contains('.') {
        return StatusCode::NOT_FOUND.into_response();
    }
    index()
}
