//! Typed application error mapped to HTTP responses.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

/// The application-wide error type returned by HTTP handlers.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("not found: {0}")]
    NotFound(String),

    #[error("bad request: {0}")]
    BadRequest(String),

    #[error("unauthorized: {0}")]
    Unauthorized(String),

    #[error("forbidden: {0}")]
    Forbidden(String),

    #[error("conflict: {0}")]
    Conflict(String),

    #[error("payment required: {0}")]
    PaymentRequired(String),

    #[error("too many requests: {0}")]
    TooManyRequests(String),

    #[error(transparent)]
    Database(#[from] sqlx::Error),

    #[error(transparent)]
    Other(#[from] anyhow::Error),
}

impl AppError {
    fn status(&self) -> StatusCode {
        match self {
            AppError::NotFound(_) => StatusCode::NOT_FOUND,
            AppError::BadRequest(_) => StatusCode::BAD_REQUEST,
            AppError::Unauthorized(_) => StatusCode::UNAUTHORIZED,
            AppError::Forbidden(_) => StatusCode::FORBIDDEN,
            AppError::Conflict(_) => StatusCode::CONFLICT,
            AppError::PaymentRequired(_) => StatusCode::PAYMENT_REQUIRED,
            AppError::TooManyRequests(_) => StatusCode::TOO_MANY_REQUESTS,
            AppError::Database(sqlx::Error::RowNotFound) => StatusCode::NOT_FOUND,
            AppError::Database(_) | AppError::Other(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let status = self.status();
        // 4xx messages are written for the caller. 5xx messages are anyhow chains
        // (provider stderr, file paths, SQL) — they go to the log, never to the
        // client.
        let message = if status == StatusCode::INTERNAL_SERVER_ERROR {
            tracing::error!(error = %self, "request failed");
            "internal error".to_string()
        } else {
            self.to_string()
        };
        let body = Json(json!({ "error": message }));
        (status, body).into_response()
    }
}

/// Convenience result alias for HTTP handlers.
pub type AppResult<T> = Result<T, AppError>;

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::to_bytes;

    async fn body_of(err: AppError) -> (StatusCode, String) {
        let resp = err.into_response();
        let status = resp.status();
        let bytes = to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        (status, String::from_utf8(bytes.to_vec()).unwrap())
    }

    #[tokio::test]
    async fn client_errors_keep_their_message() {
        let (status, body) = body_of(AppError::BadRequest("name is required".into())).await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert!(body.contains("name is required"));
    }

    #[tokio::test]
    async fn internal_errors_never_leak_their_cause() {
        let cause = anyhow::anyhow!("claude -p exited with 1: /home/takoia/.env not found");
        let (status, body) = body_of(AppError::Other(cause)).await;
        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert!(!body.contains("claude"), "body was: {body}");
        assert!(!body.contains("/home"), "body was: {body}");
        assert!(body.contains("internal error"));
    }
}
