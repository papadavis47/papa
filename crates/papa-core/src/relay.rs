//! The relay's HTTP API, from the app's side. Every request and reply is a
//! `papa-proto` type, so a change on either side is a compile error on
//! both.

use papa_proto::{
    API_PREFIX, ApiError, ChildLogIn, ChildToken, CreateShare, DatabaseToken, ErrorCode, LogIn,
    Secret, Session, SetAuthKey, Share, SignUp,
};
use reqwest::{Method, RequestBuilder, Response, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;

use crate::share::AuthKey;
use crate::{Error, Result};

/// The server a new account is made on, until the father types another.
/// For now, the development relay on this computer (see the README); a
/// public one comes with Phase 14's packaging.
pub const DEFAULT_RELAY_URL: &str = "http://127.0.0.1:8787";

/// How long a request may take before it fails. Creating a share
/// provisions a database, which is the slowest call.
const TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// A client for one relay. Cheap to clone: `reqwest::Client` shares its
/// connection pool between clones.
#[derive(Clone, Debug)]
pub struct RelayClient {
    http: reqwest::Client,
    /// `https://relay.example/v1`.
    base: String,
}

impl RelayClient {
    /// `relay_url` is the relay's address, without `/v1`.
    pub fn new(relay_url: &str) -> Result<Self> {
        Ok(Self::with_client(http_client()?, relay_url))
    }

    /// A client for `relay_url` that shares `http`'s connections.
    pub(crate) fn with_client(http: reqwest::Client, relay_url: &str) -> Self {
        Self {
            http,
            base: format!("{}{API_PREFIX}", relay_url.trim().trim_end_matches('/')),
        }
    }

    pub async fn sign_up(
        &self,
        invite_code: Secret,
        email: &str,
        password: Secret,
    ) -> Result<Session> {
        let body = SignUp {
            invite_code,
            email: email.to_owned(),
            password,
        };
        self.call(Method::POST, "/accounts", None, Some(&body))
            .await
    }

    pub async fn log_in(&self, email: &str, password: Secret) -> Result<Session> {
        let body = LogIn {
            email: email.to_owned(),
            password,
        };
        self.call(Method::POST, "/sessions", None, Some(&body))
            .await
    }

    /// Ends the session `session` belongs to.
    pub async fn log_out(&self, session: &Secret) -> Result<()> {
        self.call_empty(
            Method::DELETE,
            "/sessions/current",
            Some(session),
            None::<&()>,
        )
        .await
    }

    /// Provisions a database for a new share.
    pub async fn create_share(&self, session: &Secret, auth_key: &AuthKey) -> Result<Share> {
        let body = CreateShare {
            auth_key: auth_key.to_secret(),
        };
        self.call(Method::POST, "/shares", Some(session), Some(&body))
            .await
    }

    /// Replaces a share's key: the old one stops working at once.
    pub async fn set_auth_key(
        &self,
        session: &Secret,
        share: &str,
        auth_key: &AuthKey,
    ) -> Result<()> {
        let body = SetAuthKey {
            auth_key: auth_key.to_secret(),
        };
        let path = format!("/shares/{share}/auth-key");
        self.call_empty(Method::PUT, &path, Some(session), Some(&body))
            .await
    }

    /// Deletes a share and its database. One that's already gone counts as
    /// deleted, so stopping a share can be retried after a failure.
    pub async fn delete_share(&self, session: &Secret, share: &str) -> Result<()> {
        let path = format!("/shares/{share}");
        match self
            .call_empty(Method::DELETE, &path, Some(session), None::<&()>)
            .await
        {
            Err(Error::Relay {
                code: ErrorCode::NotFound,
                ..
            }) => Ok(()),
            result => result,
        }
    }

    /// A token that can read and write one share's database.
    pub async fn father_token(&self, session: &Secret, share: &str) -> Result<DatabaseToken> {
        let path = format!("/shares/{share}/tokens");
        self.call(Method::POST, &path, Some(session), None::<&()>)
            .await
    }

    /// What a child's app asks for: a token for its own share.
    pub async fn child_token(&self, father_email: &str, auth_key: &AuthKey) -> Result<ChildToken> {
        let body = ChildLogIn {
            father_email: father_email.to_owned(),
            auth_key: auth_key.to_secret(),
        };
        self.call(Method::POST, "/child/tokens", None, Some(&body))
            .await
    }

    async fn call<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        session: Option<&Secret>,
        body: Option<&impl Serialize>,
    ) -> Result<T> {
        let response = self.send(method, path, session, body).await?;
        response.json().await.map_err(network)
    }

    async fn call_empty(
        &self,
        method: Method,
        path: &str,
        session: Option<&Secret>,
        body: Option<&impl Serialize>,
    ) -> Result<()> {
        self.send(method, path, session, body).await.map(drop)
    }

    /// Sends a request, and turns a reply other than 2xx into
    /// [`Error::Relay`] with the code from its body.
    async fn send(
        &self,
        method: Method,
        path: &str,
        session: Option<&Secret>,
        body: Option<&impl Serialize>,
    ) -> Result<Response> {
        let mut request: RequestBuilder = self.http.request(method, format!("{}{path}", self.base));
        if let Some(session) = session {
            request = request.bearer_auth(session.expose());
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        let response = request.send().await.map_err(network)?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        // Something other than the relay (a proxy, say) may answer with a
        // body that isn't ours.
        match response.json::<ApiError>().await {
            Ok(error) => Err(Error::Relay {
                code: error.code,
                message: error.message,
            }),
            Err(_) => Err(Error::Relay {
                code: match status {
                    StatusCode::UNAUTHORIZED => ErrorCode::Unauthorized,
                    StatusCode::NOT_FOUND => ErrorCode::NotFound,
                    StatusCode::TOO_MANY_REQUESTS => ErrorCode::RateLimited,
                    _ => ErrorCode::Internal,
                },
                message: format!("the server replied {status}"),
            }),
        }
    }
}

/// An HTTP client set up for the relay. Building one loads the system's
/// root certificates, so make one and clone it.
pub(crate) fn http_client() -> Result<reqwest::Client> {
    crate::tls::init();
    reqwest::Client::builder()
        .timeout(TIMEOUT)
        .build()
        .map_err(|err| Error::Network(err.to_string()))
}

/// reqwest's errors name the URL, which holds no secrets: tokens travel in
/// headers and bodies.
fn network(err: reqwest::Error) -> Error {
    let reason = if err.is_connect() {
        "couldn't connect".to_owned()
    } else if err.is_timeout() {
        "it took too long to answer".to_owned()
    } else {
        err.to_string()
    };
    Error::Network(reason)
}
