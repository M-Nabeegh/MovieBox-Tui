use axum::{
    Json, Router,
    body::Bytes,
    extract::State,
    http::{
        HeaderMap, HeaderValue, StatusCode,
        header::{CACHE_CONTROL, HOST, ORIGIN, SET_COOKIE},
    },
    response::{IntoResponse, Response},
    routing::{get, post},
};
use serde::{Deserialize, Serialize};

use crate::server::{error::ApiError, state::AppState};

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/auth/login", post(login))
        .route("/auth/logout", post(logout))
        .route("/auth/session", get(session))
}

#[derive(Debug, Deserialize)]
struct LoginRequest {
    password: String,
}

#[derive(Debug, Serialize)]
struct SessionResponse {
    authenticated: bool,
    username: String,
    csrf_token: String,
}

async fn login(
    State(state): State<AppState>,
    headers: HeaderMap,
    body: Bytes,
) -> Result<Response, ApiError> {
    state
        .auth()
        .validate_origin(header_str(&headers, ORIGIN), header_str(&headers, HOST))?;
    let request = parse_login_request(&body)?;
    let source = state.auth().source_from_headers(
        header_str_name(&headers, "x-forwarded-for"),
        header_str_name(&headers, "x-real-ip"),
    );
    let cookie = state
        .auth()
        .login(state.pool(), &source, &request.password)
        .await?;

    let mut response = StatusCode::NO_CONTENT.into_response();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid set-cookie"),
    );
    apply_no_store(response.headers_mut());
    Ok(response)
}

async fn logout(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    state
        .auth()
        .validate_origin(header_str(&headers, ORIGIN), header_str(&headers, HOST))?;
    let session = state
        .auth()
        .authenticate(state.pool(), header_str_name(&headers, "cookie"))
        .await?;
    state.auth().validate_csrf(
        header_str_name(&headers, "x-csrf-token"),
        &session.csrf_token,
    )?;
    state
        .auth()
        .logout(state.pool(), &session.session_id)
        .await?;

    let mut response = StatusCode::NO_CONTENT.into_response();
    let cookie = state.auth().clear_session_cookie();
    response.headers_mut().insert(
        SET_COOKIE,
        HeaderValue::from_str(&cookie).expect("valid expired cookie"),
    );
    apply_no_store(response.headers_mut());
    Ok(response)
}

async fn session(State(state): State<AppState>, headers: HeaderMap) -> Result<Response, ApiError> {
    let session = state
        .auth()
        .authenticate(state.pool(), header_str_name(&headers, "cookie"))
        .await?;
    let body = Json(SessionResponse {
        authenticated: true,
        username: session.username,
        csrf_token: session.csrf_token,
    });
    let mut response = body.into_response();
    apply_no_store(response.headers_mut());
    Ok(response)
}

fn parse_login_request(body: &[u8]) -> Result<LoginRequest, ApiError> {
    let request: LoginRequest =
        serde_json::from_slice(body).map_err(|_| ApiError::invalid_request())?;
    if request.password.is_empty() {
        return Err(ApiError::invalid_request());
    }
    Ok(request)
}

fn header_str(headers: &HeaderMap, name: axum::http::header::HeaderName) -> Option<&str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn header_str_name<'a>(headers: &'a HeaderMap, name: &'static str) -> Option<&'a str> {
    headers.get(name).and_then(|value| value.to_str().ok())
}

fn apply_no_store(headers: &mut HeaderMap) {
    headers.insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
}
