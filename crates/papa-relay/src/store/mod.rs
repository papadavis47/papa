//! The relay's own database: accounts, invites, sessions and shares. The
//! only module that writes SQL.
//!
//! Unlike the app, the relay already runs on tokio, so it uses `turso`
//! directly instead of through an actor thread (ADR 0007). One connection
//! sits behind an async mutex: requests take turns, which is plenty for a
//! relay this size and means no two writes ever race.

use std::path::Path;

use papa_proto::Share;
use tokio::sync::Mutex;
use turso::{Builder, Connection, Database, Row};
use uuid::Uuid;

use crate::error::RelayError;

type Result<T> = std::result::Result<T, RelayError>;

/// Append only, as in the vault: a migration that has shipped never changes.
const MIGRATIONS: &[&str] = &[include_str!("0001_accounts_and_shares.sql")];

pub struct Store {
    conn: Mutex<Connection>,
    /// Kept alongside the connection so the database outlives it.
    _database: Database,
}

/// A father's share, with the database name the provisioner needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ShareRecord {
    pub id: String,
    pub database_name: String,
    pub database_url: String,
    pub created_at: i64,
}

impl From<ShareRecord> for Share {
    fn from(record: ShareRecord) -> Self {
        Self {
            id: record.id,
            database_url: record.database_url,
            created_at: record.created_at,
        }
    }
}

/// An invite as `admin invite list` shows it. The code itself isn't
/// stored, so it can't be shown again.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InviteInfo {
    pub note: String,
    pub created_at: i64,
    pub expires_at: i64,
    pub used_by_email: Option<String>,
}

impl Store {
    /// Opens (or creates) the database at `path` and migrates it.
    pub async fn open(path: &Path) -> Result<Self> {
        let path = path
            .to_str()
            .ok_or_else(|| RelayError::Internal(format!("not UTF-8: {}", path.display())))?;
        Self::build(path).await
    }

    /// A database that's gone when the store drops. For tests.
    pub async fn in_memory() -> Result<Self> {
        Self::build(":memory:").await
    }

    async fn build(path: &str) -> Result<Self> {
        let database = Builder::new_local(path).build().await?;
        let mut conn = database.connect()?;
        conn.execute("PRAGMA foreign_keys = ON", ()).await?;
        migrate(&mut conn).await?;
        Ok(Self {
            conn: Mutex::new(conn),
            _database: database,
        })
    }

    // --- Invites and accounts ---

    pub async fn add_invite(
        &self,
        code_hash: &[u8],
        note: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<()> {
        self.conn
            .lock()
            .await
            .execute(
                "INSERT INTO invite (code_hash, note, created_at, expires_at) VALUES (?1, ?2, ?3, ?4)",
                (code_hash, note, now, expires_at),
            )
            .await?;
        Ok(())
    }

    pub async fn invites(&self) -> Result<Vec<InviteInfo>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT invite.note, invite.created_at, invite.expires_at, account.email \
                 FROM invite LEFT JOIN account ON account.id = invite.used_by \
                 ORDER BY invite.created_at",
                (),
            )
            .await?;
        let mut invites = Vec::new();
        while let Some(row) = rows.next().await? {
            invites.push(InviteInfo {
                note: row.get(0)?,
                created_at: row.get(1)?,
                expires_at: row.get(2)?,
                used_by_email: row.get(3)?,
            });
        }
        Ok(invites)
    }

    /// Creates an account, using up the invite. Returns the account's id.
    /// `email` must be normalized and `password_hash` already hashed.
    pub async fn sign_up(
        &self,
        invite_hash: &[u8],
        email: &str,
        password_hash: &str,
        now: i64,
    ) -> Result<String> {
        let mut conn = self.conn.lock().await;
        let tx = conn.transaction().await?;

        let mut rows = tx
            .query(
                "SELECT 1 FROM invite WHERE code_hash = ?1 AND used_by IS NULL AND expires_at > ?2",
                (invite_hash, now),
            )
            .await?;
        let invite_ok = rows.next().await?.is_some();
        drop(rows);
        if !invite_ok {
            return Err(RelayError::InvalidInvite);
        }

        let mut rows = tx
            .query("SELECT 1 FROM account WHERE email = ?1", [email])
            .await?;
        let taken = rows.next().await?.is_some();
        drop(rows);
        if taken {
            return Err(RelayError::EmailTaken);
        }

        let id = Uuid::now_v7().to_string();
        tx.execute(
            "INSERT INTO account (id, email, password_hash, created_at) VALUES (?1, ?2, ?3, ?4)",
            (id.as_str(), email, password_hash, now),
        )
        .await?;
        tx.execute(
            "UPDATE invite SET used_by = ?2, used_at = ?3 WHERE code_hash = ?1",
            (invite_hash, id.as_str(), now),
        )
        .await?;
        // Dropping `tx` on an early return above rolls everything back.
        tx.commit().await?;
        Ok(id)
    }

    /// The account id and password hash for `email`, if it has an account.
    pub async fn login_details(&self, email: &str) -> Result<Option<(String, String)>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, password_hash FROM account WHERE email = ?1",
                [email],
            )
            .await?;
        Ok(match rows.next().await? {
            Some(row) => Some((row.get(0)?, row.get(1)?)),
            None => None,
        })
    }

    // --- Sessions ---

    /// Stores a new session, and forgets the account's expired ones.
    pub async fn add_session(
        &self,
        token_hash: &[u8],
        account_id: &str,
        now: i64,
        expires_at: i64,
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        conn.execute(
            "DELETE FROM session WHERE account_id = ?1 AND expires_at <= ?2",
            (account_id, now),
        )
        .await?;
        conn.execute(
            "INSERT INTO session (token_hash, account_id, created_at, expires_at) \
             VALUES (?1, ?2, ?3, ?4)",
            (token_hash, account_id, now, expires_at),
        )
        .await?;
        Ok(())
    }

    /// The account a session token belongs to, unless it has expired.
    pub async fn session_account(&self, token_hash: &[u8], now: i64) -> Result<Option<String>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT account_id FROM session WHERE token_hash = ?1 AND expires_at > ?2",
                (token_hash, now),
            )
            .await?;
        Ok(match rows.next().await? {
            Some(row) => Some(row.get(0)?),
            None => None,
        })
    }

    pub async fn delete_session(&self, token_hash: &[u8]) -> Result<()> {
        self.conn
            .lock()
            .await
            .execute("DELETE FROM session WHERE token_hash = ?1", [token_hash])
            .await?;
        Ok(())
    }

    // --- Shares ---

    pub async fn shares(&self, account_id: &str) -> Result<Vec<ShareRecord>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, database_name, database_url, created_at FROM share \
                 WHERE account_id = ?1 ORDER BY created_at, id",
                [account_id],
            )
            .await?;
        let mut shares = Vec::new();
        while let Some(row) = rows.next().await? {
            shares.push(share_from_row(&row)?);
        }
        Ok(shares)
    }

    pub async fn share(&self, account_id: &str, share_id: &str) -> Result<ShareRecord> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT id, database_name, database_url, created_at FROM share \
                 WHERE account_id = ?1 AND id = ?2",
                (account_id, share_id),
            )
            .await?;
        match rows.next().await? {
            Some(row) => share_from_row(&row),
            None => Err(RelayError::NotFound("share")),
        }
    }

    /// Fails if a share already uses this auth key.
    pub async fn ensure_auth_key_free(&self, auth_key_hash: &[u8]) -> Result<()> {
        check_auth_key_free(&*self.conn.lock().await, auth_key_hash).await
    }

    pub async fn add_share(
        &self,
        account_id: &str,
        share: &ShareRecord,
        auth_key_hash: &[u8],
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        check_auth_key_free(&conn, auth_key_hash).await?;
        conn.execute(
            "INSERT INTO share (id, account_id, database_name, database_url, auth_key_hash, created_at) \
             VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
            (
                share.id.as_str(),
                account_id,
                share.database_name.as_str(),
                share.database_url.as_str(),
                auth_key_hash,
                share.created_at,
            ),
        )
        .await?;
        Ok(())
    }

    pub async fn set_auth_key(
        &self,
        account_id: &str,
        share_id: &str,
        auth_key_hash: &[u8],
    ) -> Result<()> {
        let conn = self.conn.lock().await;
        check_auth_key_free(&conn, auth_key_hash).await?;
        let changed = conn
            .execute(
                "UPDATE share SET auth_key_hash = ?3 WHERE account_id = ?1 AND id = ?2",
                (account_id, share_id, auth_key_hash),
            )
            .await?;
        found(changed)
    }

    pub async fn delete_share(&self, account_id: &str, share_id: &str) -> Result<()> {
        let changed = self
            .conn
            .lock()
            .await
            .execute(
                "DELETE FROM share WHERE account_id = ?1 AND id = ?2",
                (account_id, share_id),
            )
            .await?;
        found(changed)
    }

    /// The share a child's auth key opens, provided it belongs to the
    /// father with `father_email`.
    pub async fn child_share(
        &self,
        father_email: &str,
        auth_key_hash: &[u8],
    ) -> Result<Option<ShareRecord>> {
        let conn = self.conn.lock().await;
        let mut rows = conn
            .query(
                "SELECT share.id, share.database_name, share.database_url, share.created_at \
                 FROM share JOIN account ON account.id = share.account_id \
                 WHERE share.auth_key_hash = ?1 AND account.email = ?2",
                (auth_key_hash, father_email),
            )
            .await?;
        Ok(match rows.next().await? {
            Some(row) => Some(share_from_row(&row)?),
            None => None,
        })
    }
}

/// Two shares with one key would let a child into both. Keys are random,
/// so this only happens if the app reuses one by mistake.
async fn check_auth_key_free(conn: &Connection, auth_key_hash: &[u8]) -> Result<()> {
    let mut rows = conn
        .query(
            "SELECT 1 FROM share WHERE auth_key_hash = ?1",
            [auth_key_hash],
        )
        .await?;
    if rows.next().await?.is_some() {
        return Err(RelayError::BadRequest(
            "that auth key is already in use; generate a new access key".into(),
        ));
    }
    Ok(())
}

fn share_from_row(row: &Row) -> Result<ShareRecord> {
    Ok(ShareRecord {
        id: row.get(0)?,
        database_name: row.get(1)?,
        database_url: row.get(2)?,
        created_at: row.get(3)?,
    })
}

fn found(changed: u64) -> Result<()> {
    if changed == 0 {
        Err(RelayError::NotFound("share"))
    } else {
        Ok(())
    }
}

/// Brings the schema up to date, one migration per transaction, with the
/// version in SQLite's `user_version` (as the vault does).
async fn migrate(conn: &mut Connection) -> Result<()> {
    let mut rows = conn.query("PRAGMA user_version", ()).await?;
    let current: i64 = match rows.next().await? {
        Some(row) => row.get(0)?,
        None => 0,
    };
    drop(rows);
    let latest = MIGRATIONS.len() as i64;
    if current > latest {
        return Err(RelayError::Internal(format!(
            "the relay database is at version {current}, newer than this build ({latest})"
        )));
    }
    for (version, sql) in (1..=latest)
        .zip(MIGRATIONS)
        .skip_while(|(v, _)| *v <= current)
    {
        let tx = conn.transaction().await?;
        tx.execute_batch(sql).await?;
        tx.execute_batch(&format!("PRAGMA user_version = {version}"))
            .await?;
        tx.commit().await?;
    }
    Ok(())
}
