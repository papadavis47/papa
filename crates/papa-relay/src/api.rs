//! The HTTP API: routes and handlers. Request and response bodies are the
//! `papa-proto` types; the table in its docs lists every route.
//!
//! A handler's arguments are *extractors*: axum builds each one from the
//! request before the handler runs, and replies with the extractor's error
//! if it can't. [`Father`] is ours: a handler that takes one only ever runs
//! for a signed-in father.

use axum::extract::rejection::JsonRejection;
use axum::extract::{FromRequest, FromRequestParts, Path, State};
use axum::http::StatusCode;
use axum::http::header::AUTHORIZATION;
use axum::http::request::Parts;
use axum::routing::{delete, get, post, put};
use axum::{Json, Router, middleware};
use papa_proto::{
    API_PREFIX, ChildLogIn, ChildToken, CreateShare, DatabaseToken, Health, LogIn, Secret, Session,
    SetAuthKey, Share, ShareList, SignUp,
};
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;
use uuid::Uuid;

use crate::error::RelayError;
use crate::provision::{Access, TOKEN_LIFETIME};
use crate::store::ShareRecord;
use crate::{AppState, now_ms, rate_limit, secrets};

/// How long a father stays signed in.
const SESSION_DAYS: i64 = 30;
const DAY_MS: i64 = 24 * 60 * 60 * 1000;

pub fn router(state: AppState) -> Router {
    // The routes that take a password or key, rate limited per address.
    let guarded = Router::new()
        .route("/accounts", post(sign_up))
        .route("/sessions", post(log_in))
        .route("/child/tokens", post(child_token))
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            rate_limit::limit,
        ));
    let v1 = Router::new()
        .route("/health", get(health))
        .route("/sessions/current", delete(log_out))
        .route("/shares", get(list_shares).post(create_share))
        .route("/shares/{id}", delete(delete_share))
        .route("/shares/{id}/auth-key", put(set_auth_key))
        .route("/shares/{id}/tokens", post(father_token))
        .merge(guarded);
    Router::new()
        .nest(API_PREFIX, v1)
        .fallback(|| async { RelayError::NotFound("route") })
        // Logs method, path, status and time for every request. Bodies,
        // which hold the secrets, aren't logged.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .with_state(state)
}

/// Like `axum::Json`, but a malformed body gets our JSON error, not
/// axum's plain-text one.
#[derive(FromRequest)]
#[from_request(via(axum::Json), rejection(RelayError))]
struct Body<T>(T);

impl From<JsonRejection> for RelayError {
    fn from(rejection: JsonRejection) -> Self {
        Self::BadRequest(rejection.body_text())
    }
}

/// A signed-in father, from `Authorization: Bearer <session token>`.
struct Father {
    account_id: String,
    token_hash: Vec<u8>,
}

impl FromRequestParts<AppState> for Father {
    type Rejection = RelayError;

    async fn from_request_parts(parts: &mut Parts, state: &AppState) -> Result<Self, RelayError> {
        let token = parts
            .headers
            .get(AUTHORIZATION)
            .and_then(|value| value.to_str().ok())
            .and_then(|value| value.strip_prefix("Bearer "))
            .ok_or(RelayError::Unauthorized)?;
        let token_hash = secrets::sha256(token.trim().as_bytes());
        let account_id = state
            .store
            .session_account(&token_hash, now_ms())
            .await?
            .ok_or(RelayError::Unauthorized)?;
        Ok(Self {
            account_id,
            token_hash,
        })
    }
}

async fn health(State(state): State<AppState>) -> Json<Health> {
    Json(Health {
        version: env!("CARGO_PKG_VERSION").to_owned(),
        mode: state.provisioner.mode(),
    })
}

// --- Accounts and sessions ---

async fn sign_up(
    State(state): State<AppState>,
    Body(request): Body<SignUp>,
) -> Result<(StatusCode, Json<Session>), RelayError> {
    let email = secrets::normalize_email(&request.email)?;
    let invite_hash =
        secrets::sha256(secrets::normalize_invite_code(request.invite_code.expose()).as_bytes());
    let password_hash = secrets::hash_password(request.password.expose().to_owned()).await?;
    let account_id = state
        .store
        .sign_up(&invite_hash, &email, &password_hash, now_ms())
        .await?;
    tracing::info!(account_id, "account created");
    let session = new_session(&state, &account_id).await?;
    Ok((StatusCode::CREATED, Json(session)))
}

async fn log_in(
    State(state): State<AppState>,
    Body(request): Body<LogIn>,
) -> Result<Json<Session>, RelayError> {
    let email =
        secrets::normalize_email(&request.email).map_err(|_| RelayError::InvalidCredentials)?;
    let details = state.store.login_details(&email).await?;
    let (account_id, hash) = details.unzip();
    let password = request.password.expose().to_owned();
    if !secrets::verify_password(password, hash).await? {
        return Err(RelayError::InvalidCredentials);
    }
    let account_id = account_id.ok_or(RelayError::InvalidCredentials)?;
    Ok(Json(new_session(&state, &account_id).await?))
}

async fn new_session(state: &AppState, account_id: &str) -> Result<Session, RelayError> {
    let token = secrets::new_token()?;
    let now = now_ms();
    let expires_at = now + SESSION_DAYS * DAY_MS;
    state
        .store
        .add_session(
            &secrets::sha256(token.as_bytes()),
            account_id,
            now,
            expires_at,
        )
        .await?;
    Ok(Session {
        token: Secret::new(token),
        expires_at,
    })
}

async fn log_out(State(state): State<AppState>, father: Father) -> Result<StatusCode, RelayError> {
    state.store.delete_session(&father.token_hash).await?;
    Ok(StatusCode::NO_CONTENT)
}

// --- Shares ---

async fn list_shares(
    State(state): State<AppState>,
    father: Father,
) -> Result<Json<ShareList>, RelayError> {
    let shares = state.store.shares(&father.account_id).await?;
    Ok(Json(ShareList {
        shares: shares.into_iter().map(Share::from).collect(),
    }))
}

async fn create_share(
    State(state): State<AppState>,
    father: Father,
    Body(request): Body<CreateShare>,
) -> Result<(StatusCode, Json<Share>), RelayError> {
    let auth_key_hash = secrets::sha256(&secrets::parse_auth_key(request.auth_key.expose())?);
    // Checked before provisioning, so a reused key doesn't cost a database.
    state.store.ensure_auth_key_free(&auth_key_hash).await?;

    let database_name = secrets::new_database_name()?;
    let database_url = state.provisioner.create_database(&database_name).await?;
    let record = ShareRecord {
        id: Uuid::now_v7().to_string(),
        database_name,
        database_url,
        created_at: now_ms(),
    };
    if let Err(error) = state
        .store
        .add_share(&father.account_id, &record, &auth_key_hash)
        .await
    {
        // Don't leave a database nobody knows about.
        if let Err(cleanup) = state
            .provisioner
            .delete_database(&record.database_name)
            .await
        {
            tracing::error!(%cleanup, name = record.database_name, "orphaned a database");
        }
        return Err(error);
    }
    tracing::info!(
        account_id = father.account_id,
        share_id = record.id,
        "share created"
    );
    Ok((StatusCode::CREATED, Json(record.into())))
}

async fn set_auth_key(
    State(state): State<AppState>,
    father: Father,
    Path(share_id): Path<String>,
    Body(request): Body<SetAuthKey>,
) -> Result<StatusCode, RelayError> {
    let auth_key_hash = secrets::sha256(&secrets::parse_auth_key(request.auth_key.expose())?);
    state
        .store
        .set_auth_key(&father.account_id, &share_id, &auth_key_hash)
        .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn delete_share(
    State(state): State<AppState>,
    father: Father,
    Path(share_id): Path<String>,
) -> Result<StatusCode, RelayError> {
    let record = state.store.share(&father.account_id, &share_id).await?;
    // The database goes first. If that fails the share stays listed, so
    // the father can try again; the other way round would forget a
    // database that still holds the child's notes.
    state
        .provisioner
        .delete_database(&record.database_name)
        .await?;
    state
        .store
        .delete_share(&father.account_id, &share_id)
        .await?;
    tracing::info!(account_id = father.account_id, share_id, "share deleted");
    Ok(StatusCode::NO_CONTENT)
}

async fn father_token(
    State(state): State<AppState>,
    father: Father,
    Path(share_id): Path<String>,
) -> Result<Json<DatabaseToken>, RelayError> {
    let record = state.store.share(&father.account_id, &share_id).await?;
    Ok(Json(database_token(&state, record, Access::Father).await?))
}

async fn child_token(
    State(state): State<AppState>,
    Body(request): Body<ChildLogIn>,
) -> Result<Json<ChildToken>, RelayError> {
    let email = secrets::normalize_email(&request.father_email)
        .map_err(|_| RelayError::InvalidCredentials)?;
    let auth_key = secrets::parse_auth_key(request.auth_key.expose())
        .map_err(|_| RelayError::InvalidCredentials)?;
    let record = state
        .store
        .child_share(&email, &secrets::sha256(&auth_key))
        .await?
        .ok_or(RelayError::InvalidCredentials)?;
    let share_id = record.id.clone();
    Ok(Json(ChildToken {
        share_id,
        database: database_token(&state, record, Access::Child).await?,
    }))
}

async fn database_token(
    state: &AppState,
    record: ShareRecord,
    access: Access,
) -> Result<DatabaseToken, RelayError> {
    // Taken before minting, so the reported expiry is never later than
    // the token's own.
    let expires_at = now_ms() + TOKEN_LIFETIME.as_millis() as i64;
    let token = state
        .provisioner
        .mint_token(&record.database_name, access)
        .await?;
    Ok(DatabaseToken {
        database_url: record.database_url,
        token,
        expires_at,
    })
}
