use std::borrow::Cow;
use std::collections::BTreeMap;

use axum::{
    Json,
    http::{HeaderValue, StatusCode, header::CACHE_CONTROL},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ServerError {
    #[error("server startup failed: {0}")]
    Startup(String),
}

#[derive(Debug, Clone)]
pub struct ApiError {
    status: StatusCode,
    code: &'static str,
    message: Cow<'static, str>,
    fields: BTreeMap<String, String>,
}

impl ApiError {
    pub fn invalid_request() -> Self {
        Self::new(
            StatusCode::BAD_REQUEST,
            "invalid_request",
            "The request body is invalid.",
        )
    }

    pub fn invalid_credentials() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "invalid_credentials",
            "The password is incorrect.",
        )
    }

    pub fn authentication_required() -> Self {
        Self::new(
            StatusCode::UNAUTHORIZED,
            "authentication_required",
            "Authentication is required.",
        )
    }

    pub fn invalid_origin() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "invalid_origin",
            "The request origin is not allowed.",
        )
    }

    pub fn csrf_mismatch() -> Self {
        Self::new(
            StatusCode::FORBIDDEN,
            "csrf_mismatch",
            "The CSRF token is invalid.",
        )
    }

    pub fn too_many_attempts() -> Self {
        Self::new(
            StatusCode::TOO_MANY_REQUESTS,
            "too_many_attempts",
            "Too many failed login attempts were received.",
        )
    }

    pub fn internal() -> Self {
        Self::new(
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
            "The request could not be completed.",
        )
    }

    fn new(status: StatusCode, code: &'static str, message: impl Into<Cow<'static, str>>) -> Self {
        Self {
            status,
            code,
            message: message.into(),
            fields: BTreeMap::new(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let envelope = ErrorEnvelope {
            error: ErrorBody {
                code: self.code,
                message: self.message,
                request_id: Uuid::new_v4().to_string(),
                fields: self.fields,
            },
        };
        let mut response = (self.status, Json(envelope)).into_response();
        response
            .headers_mut()
            .insert(CACHE_CONTROL, HeaderValue::from_static("no-store"));
        response
    }
}

#[derive(Debug, Serialize)]
struct ErrorEnvelope {
    error: ErrorBody,
}

#[derive(Debug, Serialize)]
struct ErrorBody {
    code: &'static str,
    message: Cow<'static, str>,
    request_id: String,
    fields: BTreeMap<String, String>,
}
