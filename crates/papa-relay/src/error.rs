//! One error type for the whole relay. Handlers return
//! `Result<_, RelayError>`, so `?` works everywhere, and axum turns the error
//! into a JSON [`ApiError`] through [`IntoResponse`].

use axum::Json;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use papa_proto::{ApiError, ErrorCode};

#[derive(Debug, thiserror::Error)]
pub enum RelayError {
    #[error("{0}")]
    BadRequest(String),
    #[error("the invite code is unknown, used or expired")]
    InvalidInvite,
    #[error("an account with that email already exists")]
    EmailTaken,
    #[error("wrong email, password or key")]
    InvalidCredentials,
    #[error("sign in first")]
    Unauthorized,
    #[error("no such {0}")]
    NotFound(&'static str),
    #[error("too many attempts; try again in a minute")]
    RateLimited,
    #[error("the database host refused: {0}")]
    Provisioning(String),
    #[error("database: {0}")]
    Database(#[from] turso::Error),
    #[error("{0}")]
    Internal(String),
}

impl RelayError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::BadRequest(_) => ErrorCode::BadRequest,
            Self::InvalidInvite => ErrorCode::InvalidInvite,
            Self::EmailTaken => ErrorCode::EmailTaken,
            Self::InvalidCredentials => ErrorCode::InvalidCredentials,
            Self::Unauthorized => ErrorCode::Unauthorized,
            Self::NotFound(_) => ErrorCode::NotFound,
            Self::RateLimited => ErrorCode::RateLimited,
            Self::Provisioning(_) => ErrorCode::ProvisioningFailed,
            Self::Database(_) | Self::Internal(_) => ErrorCode::Internal,
        }
    }
}

impl IntoResponse for RelayError {
    fn into_response(self) -> Response {
        let code = self.code();
        // Details of server-side failures go to the log, not to the client.
        let message = match code {
            ErrorCode::Internal => {
                tracing::error!(error = %self, "request failed");
                "something went wrong on the relay".to_owned()
            }
            ErrorCode::ProvisioningFailed => {
                tracing::error!(error = %self, "provisioning failed");
                "the database host refused; try again later".to_owned()
            }
            _ => self.to_string(),
        };
        let status =
            StatusCode::from_u16(code.status()).unwrap_or(StatusCode::INTERNAL_SERVER_ERROR);
        (status, Json(ApiError { code, message })).into_response()
    }
}
