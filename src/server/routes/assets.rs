use axum::{
    Router,
    body::Body,
    extract::Path,
    http::{
        HeaderValue, StatusCode,
        header::{
            CACHE_CONTROL, CONTENT_SECURITY_POLICY, CONTENT_TYPE, REFERRER_POLICY,
            X_CONTENT_TYPE_OPTIONS,
        },
    },
    response::Response,
    routing::get,
};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "web/dist/"]
struct WebAssets;

pub fn router() -> Router<crate::server::state::AppState> {
    Router::new()
        .route("/", get(index))
        .route("/{*path}", get(asset))
}

async fn index() -> Response {
    asset_response("index.html", true)
}
async fn asset(Path(path): Path<String>) -> Response {
    if path.starts_with("api/") {
        return StatusCode::NOT_FOUND.into_response();
    }
    let fallback = !path
        .rsplit('/')
        .next()
        .is_some_and(|name| name.contains('.'));
    asset_response(&path, fallback)
}

fn asset_response(path: &str, fallback: bool) -> Response {
    let (file, served_path) = if let Some(file) = WebAssets::get(path) {
        (file, path)
    } else if fallback {
        let Some(file) = WebAssets::get("index.html") else {
            return StatusCode::NOT_FOUND.into_response();
        };
        (file, "index.html")
    } else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut response = Response::new(Body::from(file.data.into_owned()));
    let headers = response.headers_mut();
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_str(
            mime_guess::from_path(served_path)
                .first_or_octet_stream()
                .as_ref(),
        )
        .unwrap(),
    );
    headers.insert(
        CACHE_CONTROL,
        HeaderValue::from_static(if served_path == "index.html" {
            "no-store"
        } else {
            "public, max-age=31536000, immutable"
        }),
    );
    // Poster and backdrop artwork is served from TMDB's image CDN, so it has to
    // be allowed explicitly; without it every card renders as an empty box.
    // Only that one host is added — the policy stays closed to everything else.
    headers.insert(CONTENT_SECURITY_POLICY, HeaderValue::from_static("default-src 'self'; style-src 'self' 'unsafe-inline' https://fonts.googleapis.com; font-src 'self' https://fonts.gstatic.com; img-src 'self' data: https://image.tmdb.org; script-src 'self'; frame-ancestors 'none'; base-uri 'self'; form-action 'self'"));
    headers.insert(X_CONTENT_TYPE_OPTIONS, HeaderValue::from_static("nosniff"));
    headers.insert(REFERRER_POLICY, HeaderValue::from_static("no-referrer"));
    response
}

use axum::response::IntoResponse;
