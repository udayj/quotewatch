use super::oauth_tokens::TokenCipher;
use crate::domain::prepared_run::PreparedRunError;
use crate::domain::types::{GmailThread, ReportItem, RunCostSummary, ThreadState};
use crate::integrations::gmail::{OAuthTokenStore, OAuthTokenStoreResult, StoredToken};
use chrono::{DateTime, NaiveDate, Utc};
use libsql::{Builder, Connection, Database as LibsqlDatabase, Row, params, params::IntoParams};
use std::collections::HashMap;
use std::env;
use std::path::Path;
use thiserror::Error;
use tokio::runtime::Runtime;

mod migrations;
mod retry;
mod run_commit;
mod sql;

#[cfg(test)]
mod refresh_tests;

use retry::retry_database_operation;

pub type DatabaseResult<T> = std::result::Result<T, DatabaseError>;

#[derive(Debug, Error)]
pub enum DatabaseError {
    #[error("libSQL error: {0}")]
    Libsql(#[from] libsql::Error),

    #[error("Tokio runtime error: {0}")]
    Runtime(std::io::Error),

    #[error("JSON error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("date parse error: {0}")]
    Date(#[from] chrono::ParseError),

    #[error("OAuth token storage error: {0}")]
    Token(#[from] super::oauth_tokens::TokenStoreError),

    #[error("prepared run error: {0}")]
    PreparedRun(#[from] PreparedRunError),

    #[error("database operation {operation} failed after {attempts} attempts: {source}")]
    RetryExhausted {
        operation: &'static str,
        attempts: usize,
        #[source]
        source: Box<DatabaseError>,
    },

    #[error("missing environment variable {0}")]
    MissingEnv(&'static str),

    #[error("stored state error: {0}")]
    State(String),
}

pub struct Database {
    rt: Runtime,
    conn: Connection,
    cipher: Option<TokenCipher>,
}

fn open_local_database(rt: &Runtime, path: &Path) -> DatabaseResult<LibsqlDatabase> {
    retry_database_operation("open local database", || {
        rt.block_on(async {
            Builder::new_local(path.to_string_lossy().to_string())
                .build()
                .await
        })
        .map_err(Into::into)
    })
}

impl Database {
    // Opens the configured database, prepares the optional token cipher, and
    // runs additive schema migrations. Called once per CLI command.
    pub fn open(path: &Path) -> DatabaseResult<Self> {
        let rt = Runtime::new().map_err(DatabaseError::Runtime)?;
        let db = if let Ok(url) = env::var("DATABASE_URL")
            && !url.trim().is_empty()
        {
            let auth_token = env::var("TURSO_AUTH_TOKEN")
                .map_err(|_| DatabaseError::MissingEnv("TURSO_AUTH_TOKEN"))?;
            retry_database_operation("open remote database", || {
                rt.block_on(async {
                    Builder::new_remote(url.trim().to_string(), auth_token.trim().to_string())
                        .build()
                        .await
                })
                .map_err(Into::into)
            })?
        } else {
            open_local_database(&rt, path)?
        };
        Self::from_libsql_database(rt, db)
    }

    #[cfg(test)]
    pub(crate) fn open_local_for_test(path: &Path) -> DatabaseResult<Self> {
        let rt = Runtime::new().map_err(DatabaseError::Runtime)?;
        let db = open_local_database(&rt, path)?;
        Self::from_libsql_database(rt, db)
    }

    fn from_libsql_database(rt: Runtime, db: LibsqlDatabase) -> DatabaseResult<Self> {
        let conn =
            retry_database_operation("connect database", || db.connect().map_err(Into::into))?;
        let database = Self {
            rt,
            conn,
            cipher: TokenCipher::from_env()?,
        };
        migrations::migrate(&database)?;
        Ok(database)
    }

    // Reporting -------------------------------------------------------------

    // Returns currently actionable report rows: quotation replies pending and
    // follow-ups pending, excluding expired rows.
    pub fn report_items(&self) -> DatabaseResult<Vec<ReportItem>> {
        self.query_collect(sql::REPORT_ITEMS, (), report_item_from_row)
    }

    // Returns token/cost/runtime information for the latest run so reports can
    // show operational cost.
    pub fn latest_run_cost_summary(&self) -> DatabaseResult<Option<RunCostSummary>> {
        self.query_optional(sql::LATEST_RUN_COST_SUMMARY, (), |row| {
            let started_at: String = row.get(0)?;
            let finished_at: Option<String> = row.get(1)?;
            let elapsed_seconds = elapsed_seconds(&started_at, finished_at.as_deref());
            Ok(RunCostSummary {
                input_tokens: row.get(2)?,
                cached_input_tokens: row.get(3)?,
                cache_creation_input_tokens: row.get(4)?,
                output_tokens: row.get(5)?,
                cost_inr: row.get(6)?,
                elapsed_seconds,
            })
        })
    }

    // Returns the latest run's as-of date, used for report email subjects.
    pub fn latest_run_date(&self) -> DatabaseResult<Option<NaiveDate>> {
        self.query_optional(sql::LATEST_RUN_DATE, (), |row| row.get::<String>(0))?
            .map(|value| NaiveDate::parse_from_str(&value, "%Y-%m-%d").map_err(Into::into))
            .transpose()
    }

    // Active thread refresh -------------------------------------------------

    /// Returns pending threads that have not expired on the processing date.
    /// Expired rows remain stored for history and can reappear in daily searches.
    pub fn active_thread_ids(&self, as_of_date: NaiveDate) -> DatabaseResult<Vec<String>> {
        self.query_collect(
            sql::ACTIVE_THREAD_IDS,
            params![as_of_date.to_string()],
            |row| row.get::<String>(0),
        )
    }

    /// Returns unexpired pending snapshots for replay without live Gmail access.
    pub fn active_stored_threads(&self, as_of_date: NaiveDate) -> DatabaseResult<Vec<GmailThread>> {
        let raw_threads = self.query_collect(
            sql::ACTIVE_RAW_THREADS,
            params![as_of_date.to_string()],
            |row| row.get::<String>(0),
        )?;
        raw_threads
            .iter()
            .map(|raw| {
                serde_json::from_str(raw).map_err(|err| {
                    DatabaseError::State(format!("parse stored raw Gmail thread: {err}"))
                })
            })
            .collect()
    }

    /// Returns unexpired pending digests for detecting changed threads.
    pub fn active_digest_json_by_thread_id(
        &self,
        as_of_date: NaiveDate,
    ) -> DatabaseResult<HashMap<String, String>> {
        let rows = self.query_collect(
            sql::ACTIVE_DIGESTS,
            params![as_of_date.to_string()],
            |row| Ok((row.get::<String>(0)?, row.get::<String>(1)?)),
        )?;
        Ok(rows.into_iter().collect())
    }

    // Exposes DB-backed token storage only when encryption is configured.
    // Without a cipher, Gmail OAuth falls back to env/file token sources.
    pub fn token_store(&self) -> Option<&dyn OAuthTokenStore> {
        self.cipher.as_ref().map(|_| self as &dyn OAuthTokenStore)
    }

    // Classification reads --------------------------------------------------

    // Reads the current stored state for preparation-time transition decisions.
    pub fn thread_state(&self, thread_id: &str) -> DatabaseResult<Option<ThreadState>> {
        self.query_optional(sql::PREVIOUS_STATE, params![thread_id], |row| {
            row.get::<String>(0)
        })?
        .map(|state| Ok(ThreadState::from_db_value(&state)))
        .transpose()
    }

    // Encrypted OAuth token rows -------------------------------------------

    // Reads the encrypted token payload and nonce for a token key such as
    // `gmail_read:account@example.com`.
    fn read_encrypted_token(&self, token_key: &str) -> DatabaseResult<Option<(String, String)>> {
        self.query_optional(sql::READ_ENCRYPTED_TOKEN, params![token_key], |row| {
            Ok((row.get::<String>(0)?, row.get::<String>(1)?))
        })
    }

    // Inserts or replaces the encrypted token payload for a token key after a
    // token is refreshed or bootstrapped.
    fn write_encrypted_token(
        &self,
        token_key: &str,
        nonce: &str,
        encrypted_token_json: &str,
    ) -> DatabaseResult<()> {
        self.exec(
            sql::WRITE_ENCRYPTED_TOKEN,
            params![
                token_key,
                nonce,
                encrypted_token_json,
                Utc::now().to_rfc3339()
            ],
        )?;
        Ok(())
    }

    // Query primitives ------------------------------------------------------

    // Runs one mutating SQL statement through libSQL's async API. The rest of
    // the app stays synchronous because the CLI has no concurrent DB workload.
    pub(crate) fn exec<P: IntoParams>(&self, sql: &str, params: P) -> DatabaseResult<u64> {
        let params = params.into_params()?;
        retry_database_operation("execute SQL", || {
            self.rt
                .block_on(async { self.conn.execute(sql, params.clone()).await })
                .map_err(Into::into)
        })
    }

    // Runs schema batches such as CREATE TABLE statements during migration.
    pub(crate) fn batch(&self, sql: &str) -> DatabaseResult<()> {
        retry_database_operation("execute SQL batch", || {
            self.rt
                .block_on(async { self.conn.execute_batch(sql).await })
                .map(|_| ())
                .map_err(Into::into)
        })
    }

    // Runs a query expected to return zero or one row and maps that row into a
    // domain/storage value.
    fn query_optional<T, P, F>(&self, sql: &str, params: P, map: F) -> DatabaseResult<Option<T>>
    where
        P: IntoParams,
        F: Fn(Row) -> libsql::Result<T>,
    {
        let params = params.into_params()?;
        retry_database_operation("query optional row", || {
            let mut rows = self
                .rt
                .block_on(async { self.conn.query(sql, params.clone()).await })?;
            let row = self.rt.block_on(async { rows.next().await })?;
            row.map(&map).transpose().map_err(Into::into)
        })
    }

    // Runs a query returning many rows and maps each row into a collection.
    pub(crate) fn query_collect<T, P, F>(
        &self,
        sql: &str,
        params: P,
        map: F,
    ) -> DatabaseResult<Vec<T>>
    where
        P: IntoParams,
        F: Fn(Row) -> libsql::Result<T>,
    {
        let params = params.into_params()?;
        retry_database_operation("query rows", || {
            let mut rows = self
                .rt
                .block_on(async { self.conn.query(sql, params.clone()).await })?;
            let mut out = Vec::new();
            while let Some(row) = self.rt.block_on(async { rows.next().await })? {
                out.push(map(row)?);
            }
            Ok(out)
        })
    }
}

impl OAuthTokenStore for Database {
    // Gmail OAuth calls this before env/file bootstrap. If no cipher is
    // configured or no row exists, returning None lets OAuth continue fallback
    // lookup instead of treating DB storage as mandatory.
    fn load_token(&self, token_key: &str) -> OAuthTokenStoreResult<Option<StoredToken>> {
        let Some(cipher) = &self.cipher else {
            return Ok(None);
        };
        let Some((nonce, encrypted)) = self
            .read_encrypted_token(token_key)
            .map_err(|err| err.to_string())?
        else {
            return Ok(None);
        };
        cipher
            .decrypt_token(&nonce, &encrypted)
            .map(Some)
            .map_err(|err| err.to_string())
    }

    // Gmail OAuth calls this after bootstrap, refresh, or first-time consent.
    // When encryption is disabled this is a no-op so local file/env token flows
    // can still work without DB token persistence.
    fn save_token(&self, token_key: &str, token: &StoredToken) -> OAuthTokenStoreResult<()> {
        let Some(cipher) = &self.cipher else {
            return Ok(());
        };
        let encrypted = cipher.encrypt_token(token).map_err(|err| err.to_string())?;
        self.write_encrypted_token(token_key, &encrypted.nonce, &encrypted.ciphertext)
            .map_err(|err| err.to_string())
    }
}

// Converts the report SQL projection into the shape consumed by reporting.
fn report_item_from_row(row: Row) -> libsql::Result<ReportItem> {
    Ok(ReportItem {
        thread_id: row.get(0)?,
        account: row.get(1)?,
        state: ThreadState::from_db_value(row.get::<String>(2)?.as_str()),
        requester_name: row.get(3)?,
        requester_email: row.get(4)?,
        requester_org: row.get(5)?,
        subject: row.get(6)?,
        latest_customer_message_at: row.get(7)?,
        latest_company_message_at: row.get(8)?,
        quote_sent_at: row.get(9)?,
        extracted_requirement: row.get(10)?,
        short_reason: row.get(11)?,
        confidence: row.get::<f64>(12)? as f32,
        expires_on: row.get(13)?,
    })
}

// Derives runtime from stored timestamps for report summaries. Parse failures
// produce None because cost reporting should not break the main report.
fn elapsed_seconds(started_at: &str, finished_at: Option<&str>) -> Option<i64> {
    let finished_at = finished_at?;
    let started_at = DateTime::parse_from_rfc3339(started_at).ok()?;
    let finished_at = DateTime::parse_from_rfc3339(finished_at).ok()?;
    Some((finished_at - started_at).num_seconds().max(0))
}
