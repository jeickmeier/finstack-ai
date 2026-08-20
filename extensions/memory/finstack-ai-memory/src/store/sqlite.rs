//! SQLite-backed durable [`MemoryStore`], gated behind the `sqlite` feature.
//!
//! A single [`rusqlite::Connection`] guarded by a [`Mutex`] backs the store,
//! mirroring the simple-path connection idiom used elsewhere in the
//! workspace (see `finstack-ai-store-sqlite`): WAL journaling where the
//! backing file supports it, a busy timeout for lock contention, and a
//! `PRAGMA user_version` schema guard applied once at open. Every mutation
//! runs inside one transaction so the idempotency-key claim and the record
//! write commit or roll back together.

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use finstack_ai_kernel::{ArtifactRef, Sensitivity, Timestamp};
use finstack_ai_runtime::PortFuture;

use crate::record::{
    MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope, RetentionPolicy,
};

use super::{
    MatchEvidence, MemoryHit, MemoryListing, MemoryPage, MemoryQuery, MemoryStore,
    MemoryStoreError, PutOutcome,
};

/// Applied `PRAGMA user_version` for the v1 schema. Other versions fail closed.
const SCHEMA_USER_VERSION: i32 = 1;

/// Lock-wait applied to every opened connection before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Over-fetch multiplier applied to full-text search before scope filtering
/// and truncation to the caller's requested limit.
const SEARCH_OVERFETCH_FACTOR: usize = 4;

const V1_DDL: &str = "
CREATE TABLE memory_records (
  id TEXT PRIMARY KEY,
  tenant TEXT NOT NULL,
  user TEXT, agent TEXT, workspace TEXT,
  body_inline TEXT,
  blob_ref_json TEXT,
  preview TEXT NOT NULL,
  sensitivity TEXT NOT NULL,
  keywords_json TEXT NOT NULL,
  provenance_json TEXT NOT NULL,
  created_at INTEGER NOT NULL,
  last_confirmed_at INTEGER NOT NULL,
  supersedes TEXT, superseded_by TEXT,
  retention_json TEXT NOT NULL,
  tombstoned INTEGER NOT NULL DEFAULT 0
);
CREATE INDEX memory_records_tenant ON memory_records(tenant);
CREATE VIRTUAL TABLE memory_fts USING fts5(
  id UNINDEXED, preview, body, keywords
);
CREATE TABLE memory_idempotency (
  key TEXT PRIMARY KEY,
  applied_at INTEGER NOT NULL
);
";

/// Durable [`MemoryStore`] backed by a single `SQLite` connection with an
/// FTS5 full-text index.
///
/// Available only on native targets with the `sqlite` feature enabled.
pub struct SqliteMemoryStore {
    connection: Mutex<Connection>,
}

impl SqliteMemoryStore {
    /// Open (creating if absent) a file-backed store at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] if the connection cannot be
    /// opened, pragmas cannot be applied, or the schema is missing/mismatched.
    pub fn open(path: &Path) -> Result<Self, MemoryStoreError> {
        let connection = Connection::open(path).map_err(|_| sqlite_unavailable())?;
        Self::from_connection(connection, false)
    }

    /// Open a private in-memory store (not shared across connections).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] if the connection cannot be
    /// opened, pragmas cannot be applied, or the schema is missing/mismatched.
    pub fn open_in_memory() -> Result<Self, MemoryStoreError> {
        let connection = Connection::open_in_memory().map_err(|_| sqlite_unavailable())?;
        Self::from_connection(connection, true)
    }

    fn from_connection(connection: Connection, memory: bool) -> Result<Self, MemoryStoreError> {
        connection
            .busy_timeout(BUSY_TIMEOUT)
            .map_err(|_| sqlite_unavailable())?;
        if !memory {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| sqlite_unavailable())?;
        }
        connection
            .pragma_update(None, "synchronous", "NORMAL")
            .map_err(|_| sqlite_unavailable())?;
        apply_schema(&connection)?;
        Ok(Self {
            connection: Mutex::new(connection),
        })
    }
}

fn apply_schema(connection: &Connection) -> Result<(), MemoryStoreError> {
    let version: i32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| sqlite_unavailable())?;
    match version {
        0 => {
            connection
                .execute_batch(V1_DDL)
                .map_err(|_| sqlite_unavailable())?;
            connection
                .pragma_update(None, "user_version", SCHEMA_USER_VERSION)
                .map_err(|_| sqlite_unavailable())?;
            Ok(())
        }
        SCHEMA_USER_VERSION => Ok(()),
        _ => Err(MemoryStoreError::Unavailable {
            message: Arc::from("memory_store_sqlite_schema_unsupported"),
        }),
    }
}

/// Stable, non-secret error for any `SQLite` failure. Never carries the
/// underlying `rusqlite` error text, which may embed file paths or content.
fn sqlite_unavailable() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory_store_sqlite_unavailable"),
    }
}

fn lock_error() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory_store_sqlite_lock_failed"),
    }
}

/// Map any decode error encountered while reconstructing a [`MemoryRecord`]
/// from a stored row into a `rusqlite` row-conversion error.
fn row_error<E>(_: E) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        0,
        rusqlite::types::Type::Text,
        Box::new(std::fmt::Error),
    )
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(inner, _)
            if inner.code == rusqlite::ErrorCode::ConstraintViolation
    )
}

fn sensitivity_to_text(sensitivity: Sensitivity) -> Result<String, MemoryStoreError> {
    serde_json::to_string(&sensitivity).map_err(|_| sqlite_unavailable())
}

/// Quote each whitespace-separated token so FTS5 query syntax (`-`, `"`,
/// `*`, column filters, …) in the caller's input is never interpreted as
/// query syntax; quoted tokens are matched as plain phrases, `ANDed` together.
fn sanitize_fts_query(text: &str) -> String {
    // Joined with OR, not FTS5's implicit AND: recall passes a whole user
    // turn here, and requiring every token to be present means a stored
    // memory essentially never matches. OR lets `bm25` rank by how much of
    // the query a row actually covers, which is why FTS5 was chosen.
    text.split_whitespace()
        .map(|token| format!("\"{}\"", token.replace('"', "\"\"")))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// One `memory_records` row, decoded back into a [`MemoryRecord`].
fn record_from_row(row: &Row<'_>) -> rusqlite::Result<MemoryRecord> {
    let id: String = row.get("id")?;
    let tenant: String = row.get("tenant")?;
    let user: Option<String> = row.get("user")?;
    let agent: Option<String> = row.get("agent")?;
    let workspace: Option<String> = row.get("workspace")?;
    let body_inline: Option<String> = row.get("body_inline")?;
    let blob_ref_json: Option<String> = row.get("blob_ref_json")?;
    let preview: String = row.get("preview")?;
    let sensitivity_text: String = row.get("sensitivity")?;
    let keywords_json: String = row.get("keywords_json")?;
    let provenance_json: String = row.get("provenance_json")?;
    let created_at: i64 = row.get("created_at")?;
    let last_confirmed_at: i64 = row.get("last_confirmed_at")?;
    let supersedes: Option<String> = row.get("supersedes")?;
    let superseded_by: Option<String> = row.get("superseded_by")?;
    let retention_json: String = row.get("retention_json")?;
    let tombstoned: i64 = row.get("tombstoned")?;

    let to_row_error = row_error::<serde_json::Error>;

    let body = if let Some(inline) = body_inline {
        MemoryBody::Inline(Arc::from(inline))
    } else if let Some(blob_json) = blob_ref_json {
        let artifact_ref: ArtifactRef = serde_json::from_str(&blob_json).map_err(to_row_error)?;
        MemoryBody::Blob(artifact_ref)
    } else {
        MemoryBody::Inline(Arc::from(""))
    };

    let sensitivity: Sensitivity = serde_json::from_str(&sensitivity_text).map_err(to_row_error)?;
    let keywords: Vec<Arc<str>> = serde_json::from_str(&keywords_json).map_err(to_row_error)?;
    let provenance: MemoryProvenance =
        serde_json::from_str(&provenance_json).map_err(to_row_error)?;
    let retention: RetentionPolicy = serde_json::from_str(&retention_json).map_err(to_row_error)?;
    let parsed_id = MemoryId::parse(&id).map_err(row_error::<crate::record::MemoryError>)?;

    Ok(MemoryRecord {
        id: parsed_id,
        scope: MemoryScope {
            tenant: Arc::from(tenant),
            user: user.map(Arc::from),
            agent: agent.map(Arc::from),
            workspace: workspace.map(Arc::from),
        },
        keywords: Arc::from(keywords),
        body,
        preview: Arc::from(preview),
        sensitivity,
        provenance,
        created_at: Timestamp::from_unix_ms(created_at).unwrap_or(finstack_ai_kernel::UNIX_EPOCH),
        last_confirmed_at: Timestamp::from_unix_ms(last_confirmed_at)
            .unwrap_or(finstack_ai_kernel::UNIX_EPOCH),
        supersedes: supersedes.and_then(|value| MemoryId::parse(&value).ok()),
        superseded_by: superseded_by.and_then(|value| MemoryId::parse(&value).ok()),
        retention,
        tombstoned: tombstoned != 0,
    })
}

fn body_columns(body: &MemoryBody) -> Result<(Option<String>, Option<String>), MemoryStoreError> {
    match body {
        MemoryBody::Inline(text) => Ok((Some(text.to_string()), None)),
        MemoryBody::Blob(artifact_ref) => {
            let json = serde_json::to_string(artifact_ref).map_err(|_| sqlite_unavailable())?;
            Ok((None, Some(json)))
        }
    }
}

fn body_text(body: &MemoryBody) -> String {
    match body {
        MemoryBody::Inline(text) => text.to_string(),
        MemoryBody::Blob(_) => String::new(),
    }
}

/// Insert (or replace) `record`'s row and FTS entry within `transaction`.
fn write_record(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
) -> Result<(), MemoryStoreError> {
    let (body_inline, blob_ref_json) = body_columns(&record.body)?;
    let sensitivity_text = sensitivity_to_text(record.sensitivity)?;
    let keywords_json =
        serde_json::to_string(&record.keywords).map_err(|_| sqlite_unavailable())?;
    let provenance_json =
        serde_json::to_string(&record.provenance).map_err(|_| sqlite_unavailable())?;
    let retention_json =
        serde_json::to_string(&record.retention).map_err(|_| sqlite_unavailable())?;

    transaction
        .execute(
            "INSERT OR REPLACE INTO memory_records
             (id, tenant, user, agent, workspace, body_inline, blob_ref_json, preview,
              sensitivity, keywords_json, provenance_json, created_at, last_confirmed_at,
              supersedes, superseded_by, retention_json, tombstoned)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17)",
            params![
                record.id.as_str(),
                record.scope.tenant.as_ref(),
                record.scope.user.as_deref(),
                record.scope.agent.as_deref(),
                record.scope.workspace.as_deref(),
                body_inline,
                blob_ref_json,
                record.preview.as_ref(),
                sensitivity_text,
                keywords_json,
                provenance_json,
                record.created_at.as_unix_ms(),
                record.last_confirmed_at.as_unix_ms(),
                record.supersedes.as_ref().map(MemoryId::as_str),
                record.superseded_by.as_ref().map(MemoryId::as_str),
                retention_json,
                i64::from(record.tombstoned),
            ],
        )
        .map_err(|_| sqlite_unavailable())?;

    transaction
        .execute(
            "DELETE FROM memory_fts WHERE id = ?1",
            params![record.id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;

    let keywords_text = record
        .keywords
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(" ");
    transaction
        .execute(
            "INSERT INTO memory_fts (id, preview, body, keywords) VALUES (?1, ?2, ?3, ?4)",
            params![
                record.id.as_str(),
                record.preview.as_ref(),
                body_text(&record.body),
                keywords_text,
            ],
        )
        .map_err(|_| sqlite_unavailable())?;

    Ok(())
}

fn delete_fts_row(transaction: &Transaction<'_>, id: &MemoryId) -> Result<(), MemoryStoreError> {
    transaction
        .execute("DELETE FROM memory_fts WHERE id = ?1", params![id.as_str()])
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

fn fetch_record(
    transaction: &Transaction<'_>,
    id: &MemoryId,
) -> Result<Option<MemoryRecord>, MemoryStoreError> {
    transaction
        .query_row(
            "SELECT * FROM memory_records WHERE id = ?1",
            params![id.as_str()],
            record_from_row,
        )
        .optional()
        .map_err(|_| sqlite_unavailable())
}

/// Report whether writing `incoming` at its id would collide with a row
/// that must not be replaced.
///
/// A live row always collides: `id` is the table's primary key, so a
/// collision with another tenant's row is still a conflict, and the caller
/// learns only that the identifier is taken. A *tombstoned* row in the
/// identical scope does not collide — that is the same owner re-remembering
/// something they forgot, and refusing it would strand the id forever, since
/// the supersession route rejects a replacement whose id equals the one it
/// supersedes.
fn record_conflicts(
    transaction: &Transaction<'_>,
    incoming: &MemoryRecord,
) -> Result<bool, MemoryStoreError> {
    let existing: Option<(bool, MemoryScope)> = transaction
        .query_row(
            "SELECT tombstoned, tenant, user, agent, workspace FROM memory_records
             WHERE id = ?1",
            params![incoming.id.as_str()],
            |row| {
                let tombstoned: i64 = row.get(0)?;
                let tenant: String = row.get(1)?;
                let user: Option<String> = row.get(2)?;
                let agent: Option<String> = row.get(3)?;
                let workspace: Option<String> = row.get(4)?;
                Ok((
                    tombstoned != 0,
                    MemoryScope {
                        tenant: Arc::from(tenant),
                        user: user.map(Arc::from),
                        agent: agent.map(Arc::from),
                        workspace: workspace.map(Arc::from),
                    },
                ))
            },
        )
        .optional()
        .map_err(|_| sqlite_unavailable())?;
    let Some((tombstoned, existing_scope)) = existing else {
        return Ok(false);
    };
    Ok(!(tombstoned && existing_scope == incoming.scope))
}

/// Claim `idempotency_key` inside `transaction`. Returns `true` when newly
/// claimed, `false` when the key was already applied.
fn claim_key(
    transaction: &Transaction<'_>,
    idempotency_key: &str,
) -> Result<bool, MemoryStoreError> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |duration| {
            i64::try_from(duration.as_millis()).unwrap_or(i64::MAX)
        });
    match transaction.execute(
        "INSERT INTO memory_idempotency (key, applied_at) VALUES (?1, ?2)",
        params![idempotency_key, now],
    ) {
        Ok(_) => Ok(true),
        Err(error) if is_constraint_violation(&error) => Ok(false),
        Err(_) => Err(sqlite_unavailable()),
    }
}

fn keyword_match(record: &MemoryRecord, keywords: &[Arc<str>]) -> Option<(u32, Arc<str>)> {
    let mut matched_count = 0_u32;
    let mut first_match: Option<Arc<str>> = None;
    for keyword in keywords {
        if record
            .keywords
            .iter()
            .any(|candidate| candidate.eq_ignore_ascii_case(keyword))
        {
            matched_count += 1;
            if first_match.is_none() {
                first_match = Some(Arc::clone(keyword));
            }
        }
    }
    first_match.map(|keyword| (matched_count, keyword))
}

/// [`MemoryQuery::ExactId`] search: a single primary-key lookup.
fn search_exact_id(
    connection: &Connection,
    scope: &MemoryScope,
    id: &MemoryId,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let record = connection
        .query_row(
            "SELECT * FROM memory_records
             WHERE id = ?1 AND tombstoned = 0 AND superseded_by IS NULL",
            params![id.as_str()],
            record_from_row,
        )
        .optional()
        .map_err(|_| sqlite_unavailable())?;
    Ok(record
        .filter(|record| scope.permits(&record.scope))
        .map(|record| MemoryHit {
            record,
            score: 100,
            matched: MatchEvidence::ExactId,
        })
        .into_iter()
        .collect())
}

/// [`MemoryQuery::Keywords`] search: scan the tenant's live rows and match
/// keywords in Rust (case-insensitive, scored by match count), mirroring
/// [`super::InProcessMemoryStore`]'s semantics exactly.
fn search_keywords(
    connection: &Connection,
    scope: &MemoryScope,
    keywords: &[Arc<str>],
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let mut statement = connection
        .prepare(
            "SELECT * FROM memory_records
             WHERE tenant = ?1 AND tombstoned = 0 AND superseded_by IS NULL",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(params![scope.tenant.as_ref()], record_from_row)
        .map_err(|_| sqlite_unavailable())?;
    let mut hits = Vec::new();
    for row in rows {
        let record = row.map_err(|_| sqlite_unavailable())?;
        if !scope.permits(&record.scope) {
            continue;
        }
        if let Some((matched_count, keyword)) = keyword_match(&record, keywords) {
            hits.push(MemoryHit {
                record,
                score: matched_count,
                matched: MatchEvidence::Keyword(keyword),
            });
        }
    }
    Ok(hits)
}

/// [`MemoryQuery::FullText`] search: FTS5 `bm25` ranking, over-fetched by
/// [`SEARCH_OVERFETCH_FACTOR`] and then scope-filtered/truncated in Rust.
/// `bm25` scores are floats; they are never stored, only used to order the
/// SQL result set, which is then mapped to a descending rank-order `u32`.
fn search_full_text(
    connection: &Connection,
    scope: &MemoryScope,
    text: &str,
    limit: usize,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let sanitized = sanitize_fts_query(text);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    let overfetch = limit.saturating_mul(SEARCH_OVERFETCH_FACTOR).max(limit);
    let overfetch = i64::try_from(overfetch).unwrap_or(i64::MAX);
    let mut statement = connection
        .prepare(
            "SELECT r.* FROM memory_fts f
             JOIN memory_records r ON r.id = f.id
             WHERE memory_fts MATCH ?1
               AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND r.tenant = ?2
             ORDER BY bm25(memory_fts)
             LIMIT ?3",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(
            params![sanitized, scope.tenant.as_ref(), overfetch],
            record_from_row,
        )
        .map_err(|_| sqlite_unavailable())?;
    let mut ranked = Vec::new();
    for row in rows {
        ranked.push(row.map_err(|_| sqlite_unavailable())?);
    }
    let total = ranked.len();
    Ok(ranked
        .into_iter()
        .enumerate()
        .filter(|(_, record)| scope.permits(&record.scope))
        .map(|(rank, record)| {
            let rank_score = u32::try_from(total.saturating_sub(rank)).unwrap_or(u32::MAX);
            MemoryHit {
                record,
                score: rank_score,
                matched: MatchEvidence::FullText,
            }
        })
        .collect())
}

impl MemoryStore for SqliteMemoryStore {
    fn put(
        &self,
        idempotency_key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        let outcome = (|| -> Result<PutOutcome, MemoryStoreError> {
            record
                .validate()
                .map_err(|_| MemoryStoreError::InvalidRecord {
                    reason: "memory_record_invalid",
                })?;
            let mut connection = self.connection.lock().map_err(|_| lock_error())?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| sqlite_unavailable())?;
            let newly_applied = claim_key(&transaction, &idempotency_key)?;
            if !newly_applied {
                transaction.commit().map_err(|_| sqlite_unavailable())?;
                return Ok(PutOutcome::AlreadyApplied);
            }
            if record_conflicts(&transaction, &record)? {
                // Returning without committing rolls the transaction back,
                // releasing the key claim above so a later legitimate write
                // under the same effect id is not treated as already applied.
                return Err(MemoryStoreError::IdConflict);
            }
            write_record(&transaction, &record)?;
            transaction.commit().map_err(|_| sqlite_unavailable())?;
            Ok(PutOutcome::Inserted)
        })();
        Box::pin(async move { outcome })
    }

    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>> {
        let result = (|| -> Result<Option<MemoryRecord>, MemoryStoreError> {
            let mut connection = self.connection.lock().map_err(|_| lock_error())?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Deferred)
                .map_err(|_| sqlite_unavailable())?;
            let record = fetch_record(&transaction, &id)?;
            transaction.commit().map_err(|_| sqlite_unavailable())?;
            Ok(record.filter(|record| scope.permits(&record.scope)))
        })();
        Box::pin(async move { result })
    }

    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        let result = (|| -> Result<Vec<MemoryHit>, MemoryStoreError> {
            let connection = self.connection.lock().map_err(|_| lock_error())?;
            let mut hits = match &query {
                MemoryQuery::ExactId(id) => search_exact_id(&connection, &scope, id)?,
                MemoryQuery::Keywords(keywords) => search_keywords(&connection, &scope, keywords)?,
                MemoryQuery::FullText(text) => search_full_text(&connection, &scope, text, limit)?,
            };
            hits.sort_by(|left, right| {
                right
                    .score
                    .cmp(&left.score)
                    .then_with(|| left.record.id.cmp(&right.record.id))
            });
            hits.truncate(limit);
            Ok(hits)
        })();
        Box::pin(async move { result })
    }

    fn forget(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let result = (|| -> Result<(), MemoryStoreError> {
            let mut connection = self.connection.lock().map_err(|_| lock_error())?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| sqlite_unavailable())?;
            let record = fetch_record(&transaction, &id)?.ok_or(MemoryStoreError::NotFound)?;
            if !scope.permits(&record.scope) {
                // Roll back without ever inserting the idempotency key, so a
                // retry after the record starts existing is not silently
                // treated as already-applied.
                return Err(MemoryStoreError::NotFound);
            }
            let newly_applied = claim_key(&transaction, &idempotency_key)?;
            if !newly_applied {
                transaction.commit().map_err(|_| sqlite_unavailable())?;
                return Ok(());
            }
            transaction
                .execute(
                    "UPDATE memory_records SET tombstoned = 1 WHERE id = ?1",
                    params![id.as_str()],
                )
                .map_err(|_| sqlite_unavailable())?;
            delete_fts_row(&transaction, &id)?;
            transaction.commit().map_err(|_| sqlite_unavailable())?;
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn correct(
        &self,
        idempotency_key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        mut replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let result = (|| -> Result<(), MemoryStoreError> {
            replacement
                .validate()
                .map_err(|_| MemoryStoreError::InvalidRecord {
                    reason: "memory_record_invalid",
                })?;
            // A record that supersedes itself would be hidden from search and
            // recall forever, with no surviving replacement to find.
            if replacement.id == old {
                return Err(MemoryStoreError::InvalidRecord {
                    reason: "memory_self_supersession",
                });
            }
            let mut connection = self.connection.lock().map_err(|_| lock_error())?;
            let transaction = connection
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| sqlite_unavailable())?;
            let old_record = fetch_record(&transaction, &old)?.ok_or(MemoryStoreError::NotFound)?;
            if !scope.permits(&old_record.scope) {
                // As in `forget`: never claim the key before this check, so
                // the key stays free for a retry once `old` exists.
                return Err(MemoryStoreError::NotFound);
            }
            // Replay of the correction that already ran short-circuits
            // before the conflict check, which would otherwise trip on the
            // replacement this very effect wrote the first time.
            let newly_applied = claim_key(&transaction, &idempotency_key)?;
            if !newly_applied {
                transaction.commit().map_err(|_| sqlite_unavailable())?;
                return Ok(());
            }
            // The replacement is a new record, so it is subject to the same
            // conflict rule as `put`: never overwrite a record that is not
            // this scope's own tombstone. Returning here rolls back, which
            // also releases the key claimed just above.
            if record_conflicts(&transaction, &replacement)? {
                return Err(MemoryStoreError::IdConflict);
            }
            replacement.supersedes = Some(old.clone());
            write_record(&transaction, &replacement)?;
            delete_fts_row(&transaction, &old)?;
            transaction
                .execute(
                    "UPDATE memory_records SET superseded_by = ?1 WHERE id = ?2",
                    params![replacement.id.as_str(), old.as_str()],
                )
                .map_err(|_| sqlite_unavailable())?;
            transaction.commit().map_err(|_| sqlite_unavailable())?;
            Ok(())
        })();
        Box::pin(async move { result })
    }

    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>> {
        let result = (|| -> Result<MemoryListing, MemoryStoreError> {
            let connection = self.connection.lock().map_err(|_| lock_error())?;
            let mut statement = connection
                .prepare("SELECT * FROM memory_records WHERE tenant = ?1 ORDER BY id")
                .map_err(|_| sqlite_unavailable())?;
            let rows = statement
                .query_map(params![scope.tenant.as_ref()], record_from_row)
                .map_err(|_| sqlite_unavailable())?;
            let mut matching = Vec::new();
            for row in rows {
                let record = row.map_err(|_| sqlite_unavailable())?;
                if scope.permits(&record.scope) {
                    matching.push(record);
                }
            }
            let total = matching.len();
            let records = matching
                .into_iter()
                .skip(page.offset)
                .take(page.limit)
                .collect();
            Ok(MemoryListing { records, total })
        })();
        Box::pin(async move { result })
    }
}
