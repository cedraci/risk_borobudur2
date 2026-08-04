use axum::http::{header, StatusCode, Uri};
use axum::response::{IntoResponse, Response};

#[derive(rust_embed::Embed)]
#[folder = "../../frontend/dist"]
struct Assets;

/// True when the embedded frontend bundle has no files (e.g. frontend was
/// never built). Used for a startup self-diagnosis warning.
pub fn assets_empty() -> bool {
    Assets::iter().next().is_none()
}

fn not_found_json() -> Response {
    (
        StatusCode::NOT_FOUND,
        [(header::CONTENT_TYPE, "application/problem+json")],
        serde_json::json!({"title": "Not Found", "status": 404}).to_string(),
    )
        .into_response()
}

pub async fn static_handler(uri: Uri) -> Response {
    let path = uri.path().trim_start_matches('/');
    let path = if path.is_empty() { "index.html" } else { path };
    if let Some(content) = Assets::get(path) {
        let mime = mime_guess::from_path(path).first_or_octet_stream();
        return ([(header::CONTENT_TYPE, mime.as_ref().to_string())], content.data).into_response();
    }
    // Unknown API routes must never fall back to the SPA shell.
    if path.starts_with("api/") {
        return not_found_json();
    }
    match Assets::get("index.html") {
        Some(content) => {
            let mime = mime_guess::from_path("index.html").first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref().to_string())], content.data).into_response()
        }
        None => not_found_json(),
    }
}
