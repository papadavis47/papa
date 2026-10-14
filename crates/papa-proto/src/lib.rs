//! Request and response types shared by the Papa app and `papa-relay`.
//!
//! Keeping them in one crate means the client and server can't drift apart:
//! a change to the API is a compile error on both sides.
//!
//! Every path is under `/v1`. A breaking change gets a new prefix, and the
//! relay serves both until old apps are gone. Timestamps are Unix
//! milliseconds, as in the vault.
//!
//! | Method | Path | Body → reply | Who |
//! |---|---|---|---|
//! | GET | `/v1/health` | → [`Health`] | anyone |
//! | POST | `/v1/accounts` | [`SignUp`] → [`Session`] | anyone with an invite |
//! | POST | `/v1/sessions` | [`LogIn`] → [`Session`] | a father |
//! | DELETE | `/v1/sessions/current` | → nothing | a father |
//! | GET | `/v1/shares` | → [`ShareList`] | a father |
//! | POST | `/v1/shares` | [`CreateShare`] → [`Share`] | a father |
//! | PUT | `/v1/shares/{id}/auth-key` | [`SetAuthKey`] → nothing | a father |
//! | DELETE | `/v1/shares/{id}` | → nothing | a father |
//! | POST | `/v1/shares/{id}/tokens` | → [`DatabaseToken`] | a father |
//! | POST | `/v1/child/tokens` | [`ChildLogIn`] → [`ChildToken`] | a child |
//!
//! "A father" means the request carries `Authorization: Bearer <token>`,
//! with the token from a [`Session`]. Failures reply with an [`ApiError`].

use std::fmt;

use serde::{Deserialize, Serialize};

/// The prefix of every path in this version of the API.
pub const API_PREFIX: &str = "/v1";

/// A password, key or token. Serialized as a plain string, but its `Debug`
/// output is `[redacted]`, so logging a request or response can't leak it.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct Secret(String);

impl Secret {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// The secret itself. Named so that every use stands out in review.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for Secret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("[redacted]")
    }
}

/// `GET /v1/health`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Health {
    /// The relay's version, from its `Cargo.toml`.
    pub version: String,
    /// Where share databases live.
    pub mode: Mode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Mode {
    /// A local `tursodb --sync-server`, for development. Nothing is checked
    /// at the database: any token works.
    Local,
    /// Turso Cloud, through its Platform API.
    Cloud,
}

/// `POST /v1/accounts`: a new father account. Needs an invite code minted
/// with `papa-relay admin invite create`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SignUp {
    pub invite_code: Secret,
    pub email: String,
    pub password: Secret,
}

/// `POST /v1/sessions`: a father logs in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct LogIn {
    pub email: String,
    pub password: Secret,
}

/// A father's session. Send `token` as `Authorization: Bearer <token>`.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub token: Secret,
    pub expires_at: i64,
}

/// `POST /v1/shares`: provisions a cloud database for one child.
///
/// `auth_key` is the 32-byte key the app derives from the child's access
/// key, as 64 hex digits. The relay stores only its hash, and a child who
/// presents it later gets a token for this share.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateShare {
    pub auth_key: Secret,
}

/// `PUT /v1/shares/{id}/auth-key`: the father regenerated the child's
/// access key. The old one stops working at once.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SetAuthKey {
    pub auth_key: Secret,
}

/// One shared child's cloud database, as the father sees it. Which child
/// it belongs to is known only to the father's vault.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Share {
    pub id: String,
    /// Where `turso` sync pushes and pulls.
    pub database_url: String,
    pub created_at: i64,
}

/// `GET /v1/shares`, oldest first.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ShareList {
    pub shares: Vec<Share>,
}

/// A short-lived token for one share database.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct DatabaseToken {
    pub database_url: String,
    pub token: Secret,
    pub expires_at: i64,
}

/// `POST /v1/child/tokens`: a child signs in with Dad's email and the
/// `auth_key` derived from their access key.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildLogIn {
    pub father_email: String,
    pub auth_key: Secret,
}

/// The child's share and a token that can read its notes and add messages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChildToken {
    pub share_id: String,
    pub database: DatabaseToken,
}

/// The body of every failed request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiError {
    pub code: ErrorCode,
    /// For people and logs; clients match on `code`.
    pub message: String,
}

/// What went wrong, for clients to match on.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ErrorCode {
    /// The request is malformed or a field is invalid. 400.
    BadRequest,
    /// The invite code is unknown, used or expired. 403.
    InvalidInvite,
    /// Another account has this email. 409.
    EmailTaken,
    /// Wrong email, password or child key. Deliberately doesn't say which. 401.
    InvalidCredentials,
    /// No session token, or an expired one. 401.
    Unauthorized,
    /// No such share in this account. 404.
    NotFound,
    /// Too many attempts from this address; try again in a minute. 429.
    RateLimited,
    /// Creating, deleting or minting a token for a database failed. 502.
    ProvisioningFailed,
    /// A bug or an outage in the relay. 500.
    Internal,
}

impl ErrorCode {
    /// The HTTP status the relay replies with.
    pub fn status(self) -> u16 {
        match self {
            Self::BadRequest => 400,
            Self::InvalidCredentials | Self::Unauthorized => 401,
            Self::InvalidInvite => 403,
            Self::NotFound => 404,
            Self::EmailTaken => 409,
            Self::RateLimited => 429,
            Self::Internal => 500,
            Self::ProvisioningFailed => 502,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn secrets_serialize_as_plain_strings_but_never_debug_print() {
        let login = LogIn {
            email: "dad@example.com".into(),
            password: Secret::new("correct horse"),
        };
        let json = serde_json::to_string(&login).unwrap();
        assert_eq!(
            json,
            r#"{"email":"dad@example.com","password":"correct horse"}"#
        );
        assert_eq!(serde_json::from_str::<LogIn>(&json).unwrap(), login);

        let debug = format!("{login:?}");
        assert!(!debug.contains("correct horse"), "{debug}");
        assert!(debug.contains("[redacted]"));
    }

    #[test]
    fn error_codes_are_snake_case_on_the_wire() {
        let error = ApiError {
            code: ErrorCode::InvalidCredentials,
            message: "nope".into(),
        };
        assert_eq!(
            serde_json::to_string(&error).unwrap(),
            r#"{"code":"invalid_credentials","message":"nope"}"#
        );
    }
}
