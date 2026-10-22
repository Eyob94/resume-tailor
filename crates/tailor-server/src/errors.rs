use std::fmt::Display;

use axum::{
    http::StatusCode,
    response::{IntoResponse, Response},
    Json,
};
use serde_json::json;
use tailor_core::errors::DbError;

use crate::MAX_UPLOAD_BYTES;

pub struct AppError {
    pub status: StatusCode,
    pub message: String,
}

impl AppError {
    pub fn bad_request(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            message: message.into(),
        }
    }

    pub fn not_found() -> Self {
        Self::not_found_msg("No master resume has been uploaded.")
    }

    pub fn not_found_msg(message: &str) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            message: message.into(),
        }
    }

    pub fn too_large() -> Self {
        Self {
            status: StatusCode::PAYLOAD_TOO_LARGE,
            message: format!(
                "That file is too large. The limit is {} MB.",
                MAX_UPLOAD_BYTES / 1024 / 1024
            ),
        }
    }

    pub fn multipart(e: axum::extract::multipart::MultipartError) -> Self {
        if e.status() == StatusCode::PAYLOAD_TOO_LARGE {
            Self::too_large()
        } else {
            Self {
                status: e.status(),
                message: e.body_text(),
            }
        }
    }

    pub fn internal(e: impl Display) -> Self {
        tracing::error!("internal error: {e}");
        Self {
            status: StatusCode::INTERNAL_SERVER_ERROR,
            message: "Something went wrong on the server.".into(),
        }
    }
}

impl From<DbError> for AppError {
    fn from(e: DbError) -> Self {
        Self::internal(e)
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        (self.status, Json(json!({ "error": self.message }))).into_response()
    }
}
