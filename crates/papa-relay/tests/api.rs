//! The relay's API end to end: a real server on a random port, spoken to
//! over HTTP with the `papa-proto` types, as the app will.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::extract::{Path as UrlPath, Query, State};
use axum::http::{HeaderMap, StatusCode};
use axum::routing::{delete, post};
use axum::{Json, Router};
use papa_proto::{
    ApiError, ChildLogIn, ChildToken, CreateShare, DatabaseToken, ErrorCode, Health, LogIn, Mode,
    Secret, Session, SetAuthKey, Share, ShareList, SignUp,
};
use papa_relay::provision::{LocalSync, TursoCloud};
use papa_relay::rate_limit::RateLimiter;
use papa_relay::{AppState, Provisioner, Shared, Store, now_ms, secrets};
use serde::Serialize;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use tokio::net::TcpListener;

const DAY_MS: i64 = 24 * 60 * 60 * 1000;

struct Relay {
    base: String,
    http: reqwest::Client,
    state: AppState,
}

impl Relay {
    async fn start(provisioner: Provisioner, auth_per_minute: u32) -> Self {
        let state = AppState::new(Shared {
            store: Store::in_memory().await.unwrap(),
            provisioner,
            rate_limiter: RateLimiter::new(auth_per_minute),
            client_ip_header: None,
        });
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        tokio::spawn(papa_relay::serve(
            listener,
            state.clone(),
            std::future::pending(),
        ));
        Self {
            base,
            http: reqwest::Client::new(),
            state,
        }
    }

    async fn local(sync_dir: &Path) -> Self {
        let local = LocalSync::new("http://127.0.0.1:8080", Some(sync_dir.to_owned()));
        Self::start(Provisioner::Local(local), 100).await
    }

    async fn invite(&self, expires_in_ms: i64) -> String {
        let code = secrets::new_invite_code().unwrap();
        let hash = secrets::sha256(secrets::normalize_invite_code(&code).as_bytes());
        let now = now_ms();
        self.state
            .store
            .add_invite(&hash, "test", now, now + expires_in_ms)
            .await
            .unwrap();
        code
    }

    async fn sign_up(&self, email: &str) -> String {
        let code = self.invite(DAY_MS).await;
        let session: Session = self
            .call(
                "POST",
                "/accounts",
                None,
                Some(&sign_up_body(&code, email, "correct horse battery")),
            )
            .await
            .unwrap();
        session.token.expose().to_owned()
    }

    async fn create_share(&self, father: &str, auth_key: &str) -> Share {
        self.call(
            "POST",
            "/shares",
            Some(father),
            Some(&CreateShare {
                auth_key: Secret::new(auth_key),
            }),
        )
        .await
        .unwrap()
    }

    async fn child_token(
        &self,
        father_email: &str,
        auth_key: &str,
    ) -> Result<ChildToken, ApiError> {
        self.call(
            "POST",
            "/child/tokens",
            None,
            Some(&ChildLogIn {
                father_email: father_email.into(),
                auth_key: Secret::new(auth_key),
            }),
        )
        .await
    }

    /// Sends a request; a 2xx reply parses as `T`, anything else as the
    /// relay's [`ApiError`], whose code must match the HTTP status.
    async fn call<T: DeserializeOwned>(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> Result<T, ApiError> {
        let response = self.send(method, path, bearer, body).await;
        let status = response.status();
        if status.is_success() {
            Ok(response.json().await.unwrap())
        } else {
            let error: ApiError = response.json().await.unwrap();
            assert_eq!(error.code.status(), status.as_u16(), "{error:?}");
            Err(error)
        }
    }

    async fn send(
        &self,
        method: &str,
        path: &str,
        bearer: Option<&str>,
        body: Option<&impl Serialize>,
    ) -> reqwest::Response {
        let method = reqwest::Method::from_bytes(method.as_bytes()).unwrap();
        let mut request = self.http.request(method, format!("{}{path}", self.base));
        if let Some(token) = bearer {
            request = request.bearer_auth(token);
        }
        if let Some(body) = body {
            request = request.json(body);
        }
        request.send().await.unwrap()
    }

    /// For replies without a body (204).
    async fn status(&self, method: &str, path: &str, bearer: Option<&str>) -> StatusCode {
        let status = self.send(method, path, bearer, None::<&()>).await.status();
        StatusCode::from_u16(status.as_u16()).unwrap()
    }
}

fn sign_up_body(code: &str, email: &str, password: &str) -> SignUp {
    SignUp {
        invite_code: Secret::new(code),
        email: email.into(),
        password: Secret::new(password),
    }
}

fn key(byte: u8) -> String {
    hex::encode([byte; 32])
}

fn code(result: Result<impl std::fmt::Debug, ApiError>) -> ErrorCode {
    result.expect_err("should have failed").code
}

#[tokio::test]
async fn a_father_shares_with_a_child_who_signs_in_with_his_email_and_her_key() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let health: Health = relay
        .call("GET", "/health", None, None::<&()>)
        .await
        .unwrap();
    assert_eq!(health.mode, Mode::Local);

    let dad = relay.sign_up("Dad@Example.com").await;
    let share = relay.create_share(&dad, &key(1)).await;
    let name = share
        .database_url
        .strip_prefix("http://127.0.0.1:8080/db/")
        .unwrap();
    assert!(name.starts_with("share-") && name.len() == 38, "{name}");

    let list: ShareList = relay
        .call("GET", "/shares", Some(&dad), None::<&()>)
        .await
        .unwrap();
    assert_eq!(list.shares, vec![share.clone()]);

    let father_token: DatabaseToken = relay
        .call(
            "POST",
            &format!("/shares/{}/tokens", share.id),
            Some(&dad),
            None::<&()>,
        )
        .await
        .unwrap();
    assert_eq!(father_token.database_url, share.database_url);
    assert!(father_token.token.expose().starts_with("local-father-"));
    assert!(father_token.expires_at > now_ms());

    // The child: Dad's email in any case, and her key.
    let child = relay
        .child_token(" dad@EXAMPLE.com", &key(1))
        .await
        .unwrap();
    assert_eq!(child.share_id, share.id);
    assert_eq!(child.database.database_url, share.database_url);
    assert!(child.database.token.expose().starts_with("local-child-"));

    // A wrong key, or the right key with someone else's email, say only
    // "invalid credentials".
    assert_eq!(
        code(relay.child_token("dad@example.com", &key(2)).await),
        ErrorCode::InvalidCredentials
    );
    assert_eq!(
        code(relay.child_token("mum@example.com", &key(1)).await),
        ErrorCode::InvalidCredentials
    );
    assert_eq!(
        code(relay.child_token("dad@example.com", "not hex").await),
        ErrorCode::InvalidCredentials
    );

    // Dad regenerates her key: the old one stops working.
    let set_key = relay
        .send(
            "PUT",
            &format!("/shares/{}/auth-key", share.id),
            Some(&dad),
            Some(&SetAuthKey {
                auth_key: Secret::new(key(3)),
            }),
        )
        .await;
    assert_eq!(set_key.status(), reqwest::StatusCode::NO_CONTENT);
    assert_eq!(
        code(relay.child_token("dad@example.com", &key(1)).await),
        ErrorCode::InvalidCredentials
    );
    assert_eq!(
        relay
            .child_token("dad@example.com", &key(3))
            .await
            .unwrap()
            .share_id,
        share.id
    );

    // Revoking deletes the database's files and the share.
    std::fs::create_dir(sync_dir.path().join(name)).unwrap();
    std::fs::create_dir(sync_dir.path().join("share-someone-else")).unwrap();
    assert_eq!(
        relay
            .status("DELETE", &format!("/shares/{}", share.id), Some(&dad))
            .await,
        StatusCode::NO_CONTENT
    );
    assert!(!sync_dir.path().join(name).exists());
    assert!(sync_dir.path().join("share-someone-else").exists());
    let list: ShareList = relay
        .call("GET", "/shares", Some(&dad), None::<&()>)
        .await
        .unwrap();
    assert!(list.shares.is_empty());
    assert_eq!(
        code(relay.child_token("dad@example.com", &key(3)).await),
        ErrorCode::InvalidCredentials
    );
}

#[tokio::test]
async fn invites_work_once_and_expire() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let invite = relay.invite(DAY_MS).await;

    let body = sign_up_body(
        &invite.to_lowercase(),
        "dad@example.com",
        "correct horse battery",
    );
    let _: Session = relay
        .call("POST", "/accounts", None, Some(&body))
        .await
        .unwrap();
    let again = sign_up_body(&invite, "uncle@example.com", "correct horse battery");
    assert_eq!(
        code(
            relay
                .call::<Session>("POST", "/accounts", None, Some(&again))
                .await
        ),
        ErrorCode::InvalidInvite
    );

    let expired = relay.invite(-1).await;
    let body = sign_up_body(&expired, "uncle@example.com", "correct horse battery");
    assert_eq!(
        code(
            relay
                .call::<Session>("POST", "/accounts", None, Some(&body))
                .await
        ),
        ErrorCode::InvalidInvite
    );

    // A taken email doesn't use up the invite.
    let fresh = relay.invite(DAY_MS).await;
    let body = sign_up_body(&fresh, "DAD@example.com", "correct horse battery");
    assert_eq!(
        code(
            relay
                .call::<Session>("POST", "/accounts", None, Some(&body))
                .await
        ),
        ErrorCode::EmailTaken
    );
    let body = sign_up_body(&fresh, "uncle@example.com", "short");
    assert_eq!(
        code(
            relay
                .call::<Session>("POST", "/accounts", None, Some(&body))
                .await
        ),
        ErrorCode::BadRequest
    );
    let body = sign_up_body(&fresh, "uncle@example.com", "correct horse battery");
    let _: Session = relay
        .call("POST", "/accounts", None, Some(&body))
        .await
        .unwrap();
}

#[tokio::test]
async fn sessions_log_in_and_out() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let first = relay.sign_up("dad@example.com").await;

    let log_in = |email: &str, password: &str| LogIn {
        email: email.into(),
        password: Secret::new(password),
    };
    let wrong = relay
        .call::<Session>(
            "POST",
            "/sessions",
            None,
            Some(&log_in("dad@example.com", "wrong horse battery")),
        )
        .await;
    assert_eq!(code(wrong), ErrorCode::InvalidCredentials);
    let nobody = relay
        .call::<Session>(
            "POST",
            "/sessions",
            None,
            Some(&log_in("nobody@example.com", "correct horse battery")),
        )
        .await;
    assert_eq!(code(nobody), ErrorCode::InvalidCredentials);
    let second: Session = relay
        .call(
            "POST",
            "/sessions",
            None,
            Some(&log_in(" DAD@example.com", "correct horse battery")),
        )
        .await
        .unwrap();

    assert_eq!(
        relay.status("GET", "/shares", None).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        relay.status("GET", "/shares", Some("not-a-session")).await,
        StatusCode::UNAUTHORIZED
    );

    // Logging out ends that session only.
    assert_eq!(
        relay
            .status("DELETE", "/sessions/current", Some(&first))
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        relay.status("GET", "/shares", Some(&first)).await,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        relay
            .status("GET", "/shares", Some(second.token.expose()))
            .await,
        StatusCode::OK
    );
}

#[tokio::test]
async fn fathers_cannot_reach_each_others_shares() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let dad = relay.sign_up("dad@example.com").await;
    let other = relay.sign_up("other@example.com").await;
    let share = relay.create_share(&dad, &key(1)).await;

    let list: ShareList = relay
        .call("GET", "/shares", Some(&other), None::<&()>)
        .await
        .unwrap();
    assert!(list.shares.is_empty());
    let token = relay
        .call::<DatabaseToken>(
            "POST",
            &format!("/shares/{}/tokens", share.id),
            Some(&other),
            None::<&()>,
        )
        .await;
    assert_eq!(code(token), ErrorCode::NotFound);
    assert_eq!(
        relay
            .status("DELETE", &format!("/shares/{}", share.id), Some(&other))
            .await,
        StatusCode::NOT_FOUND
    );
    let set_key = relay
        .send(
            "PUT",
            &format!("/shares/{}/auth-key", share.id),
            Some(&other),
            Some(&SetAuthKey {
                auth_key: Secret::new(key(9)),
            }),
        )
        .await;
    assert_eq!(set_key.status(), reqwest::StatusCode::NOT_FOUND);
    // Nor can one reuse a key the other's child already has.
    let reused = relay
        .call::<Share>(
            "POST",
            "/shares",
            Some(&other),
            Some(&CreateShare {
                auth_key: Secret::new(key(1)),
            }),
        )
        .await;
    assert_eq!(code(reused), ErrorCode::BadRequest);

    // Dad's share is untouched.
    assert_eq!(
        relay
            .child_token("dad@example.com", &key(1))
            .await
            .unwrap()
            .share_id,
        share.id
    );
}

#[tokio::test]
async fn bad_requests_get_json_errors() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let dad = relay.sign_up("dad@example.com").await;

    let not_json = relay
        .http
        .post(format!("{}/sessions", relay.base))
        .header("content-type", "application/json")
        .body("{")
        .send()
        .await
        .unwrap();
    assert_eq!(not_json.status(), reqwest::StatusCode::BAD_REQUEST);
    assert_eq!(
        not_json.json::<ApiError>().await.unwrap().code,
        ErrorCode::BadRequest
    );

    let short_key = relay
        .call::<Share>(
            "POST",
            "/shares",
            Some(&dad),
            Some(&CreateShare {
                auth_key: Secret::new("abcd"),
            }),
        )
        .await;
    assert_eq!(code(short_key), ErrorCode::BadRequest);
    assert_eq!(
        code(
            relay
                .call::<Value>("GET", "/nowhere", None, None::<&()>)
                .await
        ),
        ErrorCode::NotFound
    );
}

#[tokio::test]
async fn password_and_key_guessing_is_rate_limited() {
    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::start(
        Provisioner::Local(LocalSync::new(
            "http://127.0.0.1:8080",
            Some(sync_dir.path().to_owned()),
        )),
        3,
    )
    .await;
    for _ in 0..3 {
        assert_eq!(
            code(relay.child_token("dad@example.com", &key(1)).await),
            ErrorCode::InvalidCredentials
        );
    }
    assert_eq!(
        code(relay.child_token("dad@example.com", &key(1)).await),
        ErrorCode::RateLimited
    );
    // Routes without a secret aren't counted.
    assert_eq!(relay.status("GET", "/health", None).await, StatusCode::OK);
}

/// What the fake Platform API saw.
#[derive(Clone, Default)]
struct Seen(Arc<Mutex<Vec<Value>>>);

/// Starts a stand-in for `api.turso.tech` that records each request.
async fn fake_platform_api() -> (String, Seen) {
    async fn create(
        State(seen): State<Seen>,
        UrlPath(org): UrlPath<String>,
        headers: HeaderMap,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        let name = body["name"].as_str().unwrap().to_owned();
        seen.0.lock().unwrap().push(json!({
            "op": "create", "org": org, "auth": headers["authorization"].to_str().unwrap(), "body": body,
        }));
        Json(
            json!({ "database": { "DbId": "id", "Hostname": format!("{name}-{org}.aws-eu-west-1.turso.io"), "Name": name } }),
        )
    }
    async fn mint(
        State(seen): State<Seen>,
        UrlPath((_, name)): UrlPath<(String, String)>,
        Query(query): Query<Value>,
        Json(body): Json<Value>,
    ) -> Json<Value> {
        seen.0
            .lock()
            .unwrap()
            .push(json!({ "op": "token", "name": name, "query": query, "body": body }));
        Json(json!({ "jwt": format!("jwt-for-{name}") }))
    }
    async fn remove(
        State(seen): State<Seen>,
        UrlPath((_, name)): UrlPath<(String, String)>,
    ) -> (StatusCode, Json<Value>) {
        seen.0
            .lock()
            .unwrap()
            .push(json!({ "op": "delete", "name": name }));
        // As if someone had already deleted it in the dashboard.
        (
            StatusCode::NOT_FOUND,
            Json(json!({ "error": "could not find database" })),
        )
    }

    let seen = Seen::default();
    let app = Router::new()
        .route("/v1/organizations/{org}/databases", post(create))
        .route("/v1/organizations/{org}/databases/{name}", delete(remove))
        .route(
            "/v1/organizations/{org}/databases/{name}/auth/tokens",
            post(mint),
        )
        .with_state(seen.clone());
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr: SocketAddr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await });
    (format!("http://{addr}"), seen)
}

#[tokio::test]
async fn cloud_mode_drives_the_platform_api() {
    let (api_url, seen) = fake_platform_api().await;
    let cloud = TursoCloud::new(
        &api_url,
        "papa-org",
        "families",
        Secret::new("platform-secret"),
    );
    let relay = Relay::start(Provisioner::Cloud(cloud), 100).await;
    let health: Health = relay
        .call("GET", "/health", None, None::<&()>)
        .await
        .unwrap();
    assert_eq!(health.mode, Mode::Cloud);

    let dad = relay.sign_up("dad@example.com").await;
    let share = relay.create_share(&dad, &key(1)).await;
    let calls = seen.0.lock().unwrap().clone();
    let name = calls[0]["body"]["name"].as_str().unwrap().to_owned();
    assert_eq!(
        calls[0],
        json!({ "op": "create", "org": "papa-org", "auth": "Bearer platform-secret",
                "body": { "name": name, "group": "families" } })
    );
    assert_eq!(
        share.database_url,
        format!("https://{name}-papa-org.aws-eu-west-1.turso.io")
    );

    let father: DatabaseToken = relay
        .call(
            "POST",
            &format!("/shares/{}/tokens", share.id),
            Some(&dad),
            None::<&()>,
        )
        .await
        .unwrap();
    assert_eq!(father.token.expose(), format!("jwt-for-{name}"));
    let child = relay.child_token("dad@example.com", &key(1)).await.unwrap();
    assert_eq!(child.database.token.expose(), format!("jwt-for-{name}"));

    let calls = seen.0.lock().unwrap().clone();
    let query = json!({ "expiration": "1h", "authorization": "full-access" });
    assert_eq!(
        calls[1],
        json!({ "op": "token", "name": name, "query": query, "body": {} })
    );
    // The child's token can only read notes, and read and add messages.
    assert_eq!(
        calls[2],
        json!({ "op": "token", "name": name, "query": query, "body": { "fine_grained_permissions": [
            { "t": ["share_schema", "share_key", "shared_note", "shared_blob"], "a": ["data_read"] },
            { "t": ["message", "receipt"], "a": ["data_read", "data_add"] },
        ] } })
    );

    // A database that's already gone still counts as deleted.
    assert_eq!(
        relay
            .status("DELETE", &format!("/shares/{}", share.id), Some(&dad))
            .await,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        seen.0.lock().unwrap()[3],
        json!({ "op": "delete", "name": name })
    );
}

#[tokio::test]
async fn a_platform_api_failure_leaves_no_share_behind() {
    // Nothing listens here, so creating the database fails.
    let cloud = TursoCloud::new(
        "http://127.0.0.1:9",
        "papa-org",
        "default",
        Secret::new("t"),
    );
    let relay = Relay::start(Provisioner::Cloud(cloud), 100).await;
    let dad = relay.sign_up("dad@example.com").await;

    let failed = relay
        .call::<Share>(
            "POST",
            "/shares",
            Some(&dad),
            Some(&CreateShare {
                auth_key: Secret::new(key(1)),
            }),
        )
        .await;
    let error = failed.unwrap_err();
    assert_eq!(error.code, ErrorCode::ProvisioningFailed);
    assert!(
        !error.message.contains("127.0.0.1"),
        "details stay in the log: {error:?}"
    );
    let list: ShareList = relay
        .call("GET", "/shares", Some(&dad), None::<&()>)
        .await
        .unwrap();
    assert!(list.shares.is_empty());
}

#[cfg(unix)]
#[tokio::test]
async fn the_admin_socket_mints_invites_that_sign_up_works_with() {
    use papa_relay::admin::{self, Reply, Request};

    let sync_dir = tempfile::tempdir().unwrap();
    let relay = Relay::local(sync_dir.path()).await;
    let socket = sync_dir.path().join("relay.db.admin.sock");
    admin::listen(&socket, relay.state.clone()).unwrap();
    let mode = std::fs::metadata(&socket).unwrap().permissions();
    assert_eq!(
        std::os::unix::fs::PermissionsExt::mode(&mode) & 0o777,
        0o600
    );

    let request = Request::CreateInvite {
        note: "Uncle Bob".into(),
        days: 3,
    };
    let Some(Reply::Invite { code, expires_at }) = admin::send(&socket, &request).await.unwrap()
    else {
        panic!("expected an invite");
    };
    assert!((expires_at - now_ms() - 3 * DAY_MS).abs() < 60_000);
    let body = sign_up_body(&code, "bob@example.com", "correct horse battery");
    let _: Session = relay
        .call("POST", "/accounts", None, Some(&body))
        .await
        .unwrap();

    let Some(Reply::Invites { invites }) =
        admin::send(&socket, &Request::ListInvites).await.unwrap()
    else {
        panic!("expected invites");
    };
    assert_eq!(invites.len(), 1);
    assert_eq!(invites[0].note, "Uncle Bob");
    assert_eq!(invites[0].used_by_email.as_deref(), Some("bob@example.com"));

    // No relay listening: the caller falls back to the database.
    let nobody = sync_dir.path().join("nobody.sock");
    assert!(
        admin::send(&nobody, &Request::ListInvites)
            .await
            .unwrap()
            .is_none()
    );
}
