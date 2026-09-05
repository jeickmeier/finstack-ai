//! Record, receipt, search, and artifact-action queries.

use std::sync::Arc;

use rusqlite::{Connection, OptionalExtension, Row, Transaction, TransactionBehavior, params};

use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_kernel::{ArtifactRef, Digest, Sensitivity, Timestamp};
use finstack_ai_runtime::artifact::ArtifactScope;

use crate::record::{
    MemoryBody, MemoryId, MemoryProvenance, MemoryRecord, MemoryScope, RetentionPolicy,
};

use super::super::{
    EmbeddingSource, MatchEvidence, MemoryArtifactAction, MemoryHit, MemoryListing, MemoryPage,
    MemoryQuery, MemoryStoreError, MemoryStoreLimits, PutOutcome, artifact_transition_actions,
    embedding_source_digest, embedding_source_text, normalize_search_tokens, operation_fingerprint,
    similarity_score, validate_embedder_id, validate_idempotency_key,
    validate_new_record_lifecycle, validate_query, validate_record, validate_scope,
};

pub(super) fn sqlite_put(
    connection: &mut Connection,
    key: &str,
    record: &MemoryRecord,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<PutOutcome, MemoryStoreError> {
    validate_record(record)?;
    validate_new_record_lifecycle(record)?;
    validate_idempotency_key(key)?;
    let fingerprint = operation_fingerprint("put", record)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| sqlite_unavailable())?;
    cleanup_expired(&transaction, now, limits)?;
    if check_replay(&transaction, &record.scope, key, fingerprint)? {
        transaction.commit().map_err(|_| sqlite_unavailable())?;
        return Ok(PutOutcome::AlreadyApplied);
    }
    if record_conflicts(&transaction, record)? {
        return Err(MemoryStoreError::IdConflict);
    }
    let previous = fetch_record(&transaction, &record.scope, &record.id, now)?;
    let actions = artifact_transition_actions(key, previous.as_ref(), Some(record), now)?;
    reserve_receipt(&transaction, limits)?;
    reserve_record(&transaction, record, limits)?;
    reserve_artifact_actions(&transaction, &actions, limits)?;
    write_record(&transaction, record)?;
    enqueue_artifact_actions(&transaction, &actions, now)?;
    insert_receipt(&transaction, &record.scope, key, fingerprint, now)?;
    transaction.commit().map_err(|_| sqlite_unavailable())?;
    Ok(PutOutcome::Inserted)
}

pub(super) fn sqlite_get(
    connection: &Connection,
    scope: &MemoryScope,
    id: &MemoryId,
    now: Timestamp,
) -> Result<Option<MemoryRecord>, MemoryStoreError> {
    validate_scope(scope)?;
    let record = fetch_record(connection, scope, id, now)?;
    Ok(record.filter(|record| !record.tombstoned && record.superseded_by.is_none()))
}

pub(super) fn sqlite_search(
    connection: &Connection,
    scope: &MemoryScope,
    query: &MemoryQuery,
    limit: usize,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    validate_scope(scope)?;
    validate_query(query, limit, limits)?;
    let mut hits = match query {
        MemoryQuery::ExactId(id) => search_exact_id(connection, scope, id, now)?,
        MemoryQuery::Keywords(keywords) => {
            search_keywords(connection, scope, keywords, limit, now)?
        }
        MemoryQuery::FullText(text) => search_full_text(connection, scope, text, limit, now)?,
        MemoryQuery::Embedding {
            embedder_id,
            vector,
        } => search_embedding(connection, scope, embedder_id, vector, now)?,
    };
    hits.sort_by(|left, right| {
        right
            .score
            .cmp(&left.score)
            .then_with(|| left.record.id.cmp(&right.record.id))
    });
    hits.truncate(limit);
    Ok(hits)
}

pub(super) fn sqlite_forget(
    connection: &mut Connection,
    key: &str,
    scope: &MemoryScope,
    id: &MemoryId,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    validate_scope(scope)?;
    validate_idempotency_key(key)?;
    let fingerprint = operation_fingerprint("forget", &(scope, id))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| sqlite_unavailable())?;
    cleanup_expired(&transaction, now, limits)?;
    if check_replay(&transaction, scope, key, fingerprint)? {
        transaction.commit().map_err(|_| sqlite_unavailable())?;
        return Ok(());
    }
    let record = fetch_record(&transaction, scope, id, now)?.ok_or(MemoryStoreError::NotFound)?;
    if record.tombstoned || record.superseded_by.is_some() {
        return Err(MemoryStoreError::NotFound);
    }
    let actions = artifact_transition_actions(key, Some(&record), None, now)?;
    reserve_receipt(&transaction, limits)?;
    reserve_artifact_actions(&transaction, &actions, limits)?;
    transaction
        .execute(
            "UPDATE memory_records SET tombstoned = 1
             WHERE scope_digest = ?1 AND id = ?2",
            params![scope_key(scope)?, id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    delete_fts_row(&transaction, scope, id)?;
    delete_embedding_rows(&transaction, scope, id)?;
    enqueue_artifact_actions(&transaction, &actions, now)?;
    insert_receipt(&transaction, scope, key, fingerprint, now)?;
    transaction.commit().map_err(|_| sqlite_unavailable())?;
    Ok(())
}

pub(super) fn sqlite_correct(
    connection: &mut Connection,
    key: &str,
    scope: &MemoryScope,
    old: &MemoryId,
    replacement: &MemoryRecord,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let mut replacement = replacement.clone();
    validate_record(&replacement)?;
    validate_scope(scope)?;
    validate_idempotency_key(key)?;
    if replacement.id == *old {
        return Err(MemoryStoreError::InvalidRecord {
            reason: "memory_self_supersession",
        });
    }
    if replacement.tombstoned {
        return Err(MemoryStoreError::InvalidRecord {
            reason: "memory_record_lifecycle_not_initial",
        });
    }
    if replacement.supersedes.is_some() || replacement.superseded_by.is_some() {
        return Err(MemoryStoreError::InvalidRecord {
            reason: "memory_replacement_already_linked",
        });
    }
    let fingerprint = operation_fingerprint("correct", &(scope, old, &replacement))?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| sqlite_unavailable())?;
    cleanup_expired(&transaction, now, limits)?;
    if check_replay(&transaction, scope, key, fingerprint)? {
        transaction.commit().map_err(|_| sqlite_unavailable())?;
        return Ok(());
    }
    let old_record =
        fetch_record(&transaction, scope, old, now)?.ok_or(MemoryStoreError::NotFound)?;
    if old_record.tombstoned || old_record.superseded_by.is_some() {
        return Err(MemoryStoreError::NotFound);
    }
    if *scope != old_record.scope || replacement.scope != old_record.scope {
        return Err(MemoryStoreError::NotFound);
    }
    if record_conflicts(&transaction, &replacement)? {
        return Err(MemoryStoreError::IdConflict);
    }
    reserve_receipt(&transaction, limits)?;
    reserve_record(&transaction, &replacement, limits)?;
    let actions = artifact_transition_actions(key, Some(&old_record), Some(&replacement), now)?;
    reserve_artifact_actions(&transaction, &actions, limits)?;
    replacement.supersedes = Some(old.clone());
    write_record(&transaction, &replacement)?;
    delete_fts_row(&transaction, scope, old)?;
    // The superseded record leaves every space; the replacement's stale rows
    // were dropped by `write_record`.
    delete_embedding_rows(&transaction, scope, old)?;
    transaction
        .execute(
            "UPDATE memory_records SET superseded_by = ?1
             WHERE scope_digest = ?2 AND id = ?3",
            params![replacement.id.as_str(), scope_key(scope)?, old.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    enqueue_artifact_actions(&transaction, &actions, now)?;
    insert_receipt(&transaction, scope, key, fingerprint, now)?;
    transaction.commit().map_err(|_| sqlite_unavailable())?;
    Ok(())
}

pub(super) fn sqlite_list(
    connection: &Connection,
    scope: &MemoryScope,
    page: MemoryPage,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<MemoryListing, MemoryStoreError> {
    validate_scope(scope)?;
    if page.limit > limits.max_page_size {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_page_limit_exceeded",
        });
    }
    let total: i64 = connection
        .query_row(
            "SELECT COUNT(*) FROM memory_records
             WHERE scope_digest = ?1 AND tombstoned = 0 AND superseded_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?2)",
            params![scope_key(scope)?, now.as_unix_ms()],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    let mut statement = connection
        .prepare(
            "SELECT * FROM memory_records
             WHERE scope_digest = ?1 AND tombstoned = 0 AND superseded_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?4)
             ORDER BY id LIMIT ?2 OFFSET ?3",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(
            params![
                scope_key(scope)?,
                i64::try_from(page.limit).unwrap_or(i64::MAX),
                i64::try_from(page.offset).unwrap_or(i64::MAX),
                now.as_unix_ms(),
            ],
            record_from_row,
        )
        .map_err(|_| sqlite_unavailable())?;
    let mut records = Vec::new();
    for row in rows {
        records.push(row.map_err(|_| sqlite_unavailable())?);
    }
    Ok(MemoryListing {
        records,
        total: usize::try_from(total).unwrap_or(usize::MAX),
    })
}
/// Stable, non-secret error for any `SQLite` failure. Never carries the
/// underlying `rusqlite` error text, which may embed file paths or content.
pub(super) fn sqlite_unavailable() -> MemoryStoreError {
    MemoryStoreError::Unavailable {
        message: Arc::from("memory_store_sqlite_unavailable"),
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

/// Normalize and quote each alphanumeric token so `FTS5` syntax in caller
/// input is never interpreted. Tokens are prefix-matched and joined with `OR`.
fn sanitize_fts_query(text: &str) -> String {
    // Joined with OR, not FTS5's implicit AND: recall passes a whole user
    // turn here, and requiring every token to be present means a stored
    // memory essentially never matches. OR lets `bm25` rank by how much of
    // the query a row actually covers, which is why FTS5 was chosen.
    normalize_search_tokens(text)
        .iter()
        .map(|token| format!("\"{token}\"*"))
        .collect::<Vec<_>>()
        .join(" OR ")
}

/// One `memory_records` row, decoded back into a [`MemoryRecord`].
fn record_from_row(row: &Row<'_>) -> rusqlite::Result<MemoryRecord> {
    record_from_row_inner(row, true)
}

pub(super) fn legacy_record_from_row(row: &Row<'_>) -> rusqlite::Result<MemoryRecord> {
    record_from_row_inner(row, false)
}

fn record_from_row_inner(
    row: &Row<'_>,
    verify_scope_digest: bool,
) -> rusqlite::Result<MemoryRecord> {
    let stored_scope_digest = if verify_scope_digest {
        Some(row.get::<_, String>("scope_digest")?)
    } else {
        None
    };
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

    let body = match (body_inline, blob_ref_json) {
        (Some(inline), None) => MemoryBody::Inline(Arc::from(inline)),
        (None, Some(blob_json)) => {
            let blob_value: serde_json::Value =
                serde_json::from_str(&blob_json).map_err(to_row_error)?;
            let scope = serde_json::from_value::<ArtifactScope>(
                blob_value
                    .get("scope")
                    .cloned()
                    .ok_or_else(|| row_error(std::fmt::Error))?,
            )
            .map_err(to_row_error)?;
            let artifact = serde_json::from_value::<ArtifactRef>(
                blob_value
                    .get("artifact")
                    .cloned()
                    .ok_or_else(|| row_error(std::fmt::Error))?,
            )
            .map_err(to_row_error)?;
            MemoryBody::Blob { scope, artifact }
        }
        _ => return Err(row_error(std::fmt::Error)),
    };

    let sensitivity: Sensitivity = serde_json::from_str(&sensitivity_text).map_err(to_row_error)?;
    let keywords: Vec<Arc<str>> = serde_json::from_str(&keywords_json).map_err(to_row_error)?;
    let provenance: MemoryProvenance =
        serde_json::from_str(&provenance_json).map_err(to_row_error)?;
    let retention: RetentionPolicy = serde_json::from_str(&retention_json).map_err(to_row_error)?;
    let parsed_id = MemoryId::parse(&id).map_err(row_error::<crate::record::MemoryError>)?;

    let created_at = Timestamp::from_unix_ms(created_at).map_err(row_error)?;
    let last_confirmed_at = Timestamp::from_unix_ms(last_confirmed_at).map_err(row_error)?;
    let supersedes = supersedes
        .map(|value| MemoryId::parse(&value))
        .transpose()
        .map_err(row_error)?;
    let superseded_by = superseded_by
        .map(|value| MemoryId::parse(&value))
        .transpose()
        .map_err(row_error)?;
    let tombstoned = match tombstoned {
        0 => false,
        1 => true,
        _ => return Err(row_error(std::fmt::Error)),
    };
    let record = MemoryRecord {
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
        created_at,
        last_confirmed_at,
        supersedes,
        superseded_by,
        retention,
        tombstoned,
    };
    record.validate().map_err(row_error)?;
    if stored_scope_digest.is_some_and(|stored| {
        record
            .scope
            .digest()
            .map_or(true, |digest| digest.to_hex() != stored)
    }) {
        return Err(row_error(std::fmt::Error));
    }
    Ok(record)
}

fn body_columns(body: &MemoryBody) -> Result<(Option<String>, Option<String>), MemoryStoreError> {
    match body {
        MemoryBody::Inline(text) => Ok((Some(text.to_string()), None)),
        MemoryBody::Blob { scope, artifact } => {
            let json = serde_json::to_string(&serde_json::json!({
                "scope": scope,
                "artifact": artifact,
            }))
            .map_err(|_| sqlite_unavailable())?;
            Ok((None, Some(json)))
        }
    }
}

/// Insert (or replace) `record`'s row and FTS entry within `transaction`.
pub(super) fn write_record(
    transaction: &Transaction<'_>,
    record: &MemoryRecord,
) -> Result<(), MemoryStoreError> {
    let scope_digest = scope_key(&record.scope)?;
    let (body_inline, blob_ref_json) = body_columns(&record.body)?;
    let sensitivity_text =
        serde_json::to_string(&record.sensitivity).map_err(|_| sqlite_unavailable())?;
    let keywords_json =
        serde_json::to_string(&record.keywords).map_err(|_| sqlite_unavailable())?;
    let provenance_json =
        serde_json::to_string(&record.provenance).map_err(|_| sqlite_unavailable())?;
    let retention_json =
        serde_json::to_string(&record.retention).map_err(|_| sqlite_unavailable())?;
    let expires_at = expires_at(record)?;

    transaction
        .execute(
            "INSERT OR REPLACE INTO memory_records
             (scope_digest, id, tenant, user, agent, workspace, body_inline, blob_ref_json, preview,
              sensitivity, keywords_json, provenance_json, created_at, last_confirmed_at,
              supersedes, superseded_by, retention_json, expires_at, tombstoned)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19)",
            params![
                scope_digest,
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
                expires_at,
                i64::from(record.tombstoned),
            ],
        )
        .map_err(|_| sqlite_unavailable())?;

    transaction
        .execute(
            "DELETE FROM memory_fts WHERE scope_digest = ?1 AND id = ?2",
            params![scope_digest, record.id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    // The written content supersedes whatever any space indexed for this id
    // (e.g. a revived tombstone's stale rows).
    delete_embedding_rows(transaction, &record.scope, &record.id)?;

    let keywords_text = record
        .keywords
        .iter()
        .map(std::convert::AsRef::as_ref)
        .collect::<Vec<_>>()
        .join(" ");
    let body_text = match &record.body {
        MemoryBody::Inline(text) => text.as_ref(),
        MemoryBody::Blob { .. } => "",
    };
    transaction
        .execute(
            "INSERT INTO memory_fts (scope_digest, id, preview, body, keywords)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                scope_digest,
                record.id.as_str(),
                record.preview.as_ref(),
                body_text,
                keywords_text,
            ],
        )
        .map_err(|_| sqlite_unavailable())?;

    transaction
        .execute(
            "DELETE FROM memory_keywords WHERE scope_digest = ?1 AND id = ?2",
            params![scope_digest, record.id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    for (ordinal, keyword) in record.keywords.iter().enumerate() {
        transaction
            .execute(
                "INSERT INTO memory_keywords
                 (scope_digest, id, ordinal, keyword, keyword_folded)
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    scope_digest,
                    record.id.as_str(),
                    i64::try_from(ordinal).unwrap_or(i64::MAX),
                    keyword.as_ref(),
                    keyword.to_ascii_lowercase(),
                ],
            )
            .map_err(|_| sqlite_unavailable())?;
    }

    Ok(())
}

/// Hard-expiry instant stored in the `expires_at` column: `None` for
/// records kept until deleted.
fn expires_at(record: &MemoryRecord) -> Result<Option<i64>, MemoryStoreError> {
    match record.retention {
        RetentionPolicy::KeepUntilDeleted => Ok(None),
        RetentionPolicy::ExpireAfterMs(duration_ms) => record
            .created_at
            .checked_add(finstack_ai_kernel::Duration::from_millis(duration_ms))
            .map(|instant| Some(instant.as_unix_ms()))
            .map_err(|_| MemoryStoreError::InvalidRecord {
                reason: "retention_overflow",
            }),
    }
}

fn delete_fts_row(
    transaction: &Transaction<'_>,
    scope: &MemoryScope,
    id: &MemoryId,
) -> Result<(), MemoryStoreError> {
    transaction
        .execute(
            "DELETE FROM memory_fts WHERE scope_digest = ?1 AND id = ?2",
            params![scope_key(scope)?, id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

/// Drop every embedding row of `(scope, id)` across all spaces. Called in
/// the same transaction as every record death or rewrite so no dead vector
/// can rank.
fn delete_embedding_rows(
    transaction: &Transaction<'_>,
    scope: &MemoryScope,
    id: &MemoryId,
) -> Result<(), MemoryStoreError> {
    transaction
        .execute(
            "DELETE FROM memory_embeddings WHERE scope_digest = ?1 AND id = ?2",
            params![scope_key(scope)?, id.as_str()],
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

/// Fetch one row by `(scope, id)`, excluding hard-expired rows.
/// Write paths sweep first, so the `expires_at` predicate is redundant there.
fn fetch_record(
    connection: &Connection,
    scope: &MemoryScope,
    id: &MemoryId,
    now: Timestamp,
) -> Result<Option<MemoryRecord>, MemoryStoreError> {
    connection
        .query_row(
            "SELECT * FROM memory_records WHERE scope_digest = ?1 AND id = ?2
               AND (expires_at IS NULL OR expires_at > ?3)",
            params![scope_key(scope)?, id.as_str(), now.as_unix_ms()],
            record_from_row,
        )
        .optional()
        .map_err(|_| sqlite_unavailable())
}

/// Report whether writing `incoming` at its id would collide with a row
/// that must not be replaced.
///
/// A live row in the exact same complete scope collides. A tombstoned row in
/// that scope does not: that is the same owner re-remembering something they
/// forgot, and refusing it would strand the id forever because correction
/// rejects self-supersession.
fn record_conflicts(
    transaction: &Transaction<'_>,
    incoming: &MemoryRecord,
) -> Result<bool, MemoryStoreError> {
    let existing: Option<(bool, MemoryScope)> = transaction
        .query_row(
            "SELECT tombstoned, tenant, user, agent, workspace FROM memory_records
             WHERE scope_digest = ?1 AND id = ?2",
            params![scope_key(&incoming.scope)?, incoming.id.as_str()],
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

/// Check whether a key is new or an exact replay of the same operation.
fn receipt_replay(
    transaction: &Transaction<'_>,
    scope: &MemoryScope,
    idempotency_key: &str,
) -> Result<Option<String>, MemoryStoreError> {
    transaction
        .query_row(
            "SELECT fingerprint FROM memory_idempotency
             WHERE key = ?1 AND scope_digest IN (?2, '*')
             ORDER BY scope_digest = '*' ASC LIMIT 1",
            params![idempotency_key, scope_key(scope)?],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| sqlite_unavailable())
}

fn insert_receipt(
    transaction: &Transaction<'_>,
    scope: &MemoryScope,
    idempotency_key: &str,
    fingerprint: Digest,
    now: Timestamp,
) -> Result<(), MemoryStoreError> {
    transaction
        .execute(
            "INSERT INTO memory_idempotency
             (scope_digest, key, fingerprint, applied_at) VALUES (?1, ?2, ?3, ?4)",
            params![
                scope_key(scope)?,
                idempotency_key,
                fingerprint.to_hex(),
                now.as_unix_ms()
            ],
        )
        .map_err(|error| {
            if is_constraint_violation(&error) {
                MemoryStoreError::IdempotencyConflict
            } else {
                sqlite_unavailable()
            }
        })?;
    Ok(())
}

fn check_replay(
    transaction: &Transaction<'_>,
    scope: &MemoryScope,
    key: &str,
    fingerprint: Digest,
) -> Result<bool, MemoryStoreError> {
    match receipt_replay(transaction, scope, key)? {
        None => Ok(false),
        Some(existing) if existing == fingerprint.to_hex() => Ok(true),
        Some(_) => Err(MemoryStoreError::IdempotencyConflict),
    }
}

fn scope_key(scope: &MemoryScope) -> Result<String, MemoryStoreError> {
    scope
        .digest()
        .map(|digest| digest.to_hex())
        .map_err(|_| MemoryStoreError::InvalidRequest {
            reason: "memory_scope_invalid",
        })
}

/// Write-path sweep: drop expired rows (enqueueing artifact unpins) and
/// age out idempotency receipts. Reads filter on `expires_at` instead.
fn cleanup_expired(
    transaction: &Transaction<'_>,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let receipt_cutoff = now
        .as_unix_ms()
        .saturating_sub(i64::try_from(limits.max_receipt_age_ms).unwrap_or(i64::MAX));
    transaction
        .execute(
            "DELETE FROM memory_idempotency WHERE applied_at <= ?1",
            params![receipt_cutoff],
        )
        .map_err(|_| sqlite_unavailable())?;
    let expired = {
        let mut statement = transaction
            .prepare(
                "SELECT * FROM memory_records
                 WHERE expires_at IS NOT NULL AND expires_at <= ?1 ORDER BY id",
            )
            .map_err(|_| sqlite_unavailable())?;
        let rows = statement
            .query_map(params![now.as_unix_ms()], record_from_row)
            .map_err(|_| sqlite_unavailable())?;
        let mut records = Vec::new();
        for row in rows {
            records.push(row.map_err(|_| sqlite_unavailable())?);
        }
        records
    };
    let mut actions = Vec::new();
    for record in &expired {
        actions.extend(artifact_transition_actions(
            &format!(
                "expiry:{}:{}",
                record.id.as_str(),
                record.created_at.as_unix_ms()
            ),
            Some(record),
            None,
            now,
        )?);
    }
    reserve_artifact_actions(transaction, &actions, limits)?;
    enqueue_artifact_actions(transaction, &actions, now)?;
    transaction
        .execute(
            "DELETE FROM memory_fts WHERE (scope_digest, id) IN (
               SELECT scope_digest, id FROM memory_records
               WHERE expires_at IS NOT NULL AND expires_at <= ?1
             )",
            params![now.as_unix_ms()],
        )
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute(
            "DELETE FROM memory_keywords WHERE (scope_digest, id) IN (
               SELECT scope_digest, id FROM memory_records
               WHERE expires_at IS NOT NULL AND expires_at <= ?1
             )",
            params![now.as_unix_ms()],
        )
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute(
            "DELETE FROM memory_embeddings WHERE (scope_digest, id) IN (
               SELECT scope_digest, id FROM memory_records
               WHERE expires_at IS NOT NULL AND expires_at <= ?1
             )",
            params![now.as_unix_ms()],
        )
        .map_err(|_| sqlite_unavailable())?;
    transaction
        .execute(
            "DELETE FROM memory_records WHERE expires_at IS NOT NULL AND expires_at <= ?1",
            params![now.as_unix_ms()],
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

fn reserve_artifact_actions(
    transaction: &Transaction<'_>,
    actions: &[MemoryArtifactAction],
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM memory_artifact_outbox", [], |row| {
            row.get(0)
        })
        .map_err(|_| sqlite_unavailable())?;
    let mut additional = 0_usize;
    for action in actions {
        let exists: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM memory_artifact_outbox WHERE action_id = ?1)",
                params![action.action_id().to_hex()],
                |row| row.get(0),
            )
            .map_err(|_| sqlite_unavailable())?;
        if !exists {
            additional = additional.saturating_add(1);
        }
    }
    if usize::try_from(count)
        .unwrap_or(usize::MAX)
        .saturating_add(additional)
        > limits.max_artifact_actions
    {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "artifact_actions",
            limit: u64::try_from(limits.max_artifact_actions).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn enqueue_artifact_actions(
    transaction: &Transaction<'_>,
    actions: &[MemoryArtifactAction],
    now: Timestamp,
) -> Result<(), MemoryStoreError> {
    for action in actions {
        let encoded = serde_json::to_string(action).map_err(|_| sqlite_unavailable())?;
        transaction
            .execute(
                "INSERT OR IGNORE INTO memory_artifact_outbox
                 (action_id, action_json, created_at) VALUES (?1, ?2, ?3)",
                params![action.action_id().to_hex(), encoded, now.as_unix_ms()],
            )
            .map_err(|_| sqlite_unavailable())?;
    }
    Ok(())
}

pub(super) fn pending_artifact_actions(
    connection: &Connection,
    limit: usize,
    limits: MemoryStoreLimits,
) -> Result<Vec<MemoryArtifactAction>, MemoryStoreError> {
    if limit > limits.max_artifact_actions {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_artifact_action_limit_exceeded",
        });
    }
    let mut statement = connection
        .prepare(
            "SELECT action_json FROM memory_artifact_outbox
             ORDER BY sequence LIMIT ?1",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(params![i64::try_from(limit).unwrap_or(i64::MAX)], |row| {
            row.get::<_, String>(0)
        })
        .map_err(|_| sqlite_unavailable())?;
    let mut actions = Vec::new();
    for row in rows {
        let encoded = row.map_err(|_| sqlite_unavailable())?;
        let action = serde_json::from_str(&encoded).map_err(|_| sqlite_unavailable())?;
        actions.push(action);
    }
    Ok(actions)
}

pub(super) fn acknowledge_artifact_action(
    connection: &Connection,
    action_id: Digest,
) -> Result<(), MemoryStoreError> {
    connection
        .execute(
            "DELETE FROM memory_artifact_outbox WHERE action_id = ?1",
            params![action_id.to_hex()],
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

/// [`MemoryStore::pending_embedding_sources`] over the anti-join: live
/// records in any scope whose `memory_embeddings` row for the space is
/// absent **or** carries a source digest that no longer matches the record's
/// current content. Stable order `(scope_digest, id)`; the batch is capped
/// by [`MemoryStoreLimits::max_search_results`].
///
/// [`MemoryStore::pending_embedding_sources`]: super::super::MemoryStore::pending_embedding_sources
pub(super) fn pending_embedding_sources(
    connection: &Connection,
    embedder_id: &str,
    limit: usize,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<Vec<EmbeddingSource>, MemoryStoreError> {
    validate_embedder_id(embedder_id)?;
    let limit = limit.min(limits.max_search_results);
    let mut statement = connection
        .prepare(
            "SELECT r.*, e.source_digest AS indexed_source_digest
             FROM memory_records r
             LEFT JOIN memory_embeddings e
               ON e.scope_digest = r.scope_digest AND e.id = r.id AND e.embedder_id = ?1
             WHERE r.tombstoned = 0 AND r.superseded_by IS NULL
               AND (r.expires_at IS NULL OR r.expires_at > ?2)
             ORDER BY r.scope_digest, r.id",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(params![embedder_id, now.as_unix_ms()], |row| {
            let record = record_from_row(row)?;
            let indexed: Option<String> = row.get("indexed_source_digest")?;
            Ok((record, indexed))
        })
        .map_err(|_| sqlite_unavailable())?;
    let mut sources = Vec::new();
    for row in rows {
        if sources.len() >= limit {
            break;
        }
        let (record, indexed) = row.map_err(|_| sqlite_unavailable())?;
        let text = embedding_source_text(&record);
        let source_digest = embedding_source_digest(&text)?;
        // Anti-join on current content: a record is pending unless the space
        // holds a row for it whose stored digest still matches, so a stale
        // row re-surfaces its record.
        if indexed.is_some_and(|stored| stored == source_digest.to_hex()) {
            continue;
        }
        sources.push(EmbeddingSource {
            scope: record.scope,
            id: record.id,
            text: Arc::from(text),
            source_digest,
        });
    }
    Ok(sources)
}

/// One embedding write, exactly as [`MemoryStore::store_embedding`] received
/// it, carried whole from the worker command into the query.
///
/// [`MemoryStore::store_embedding`]: super::super::MemoryStore::store_embedding
pub(super) struct EmbeddingWrite {
    pub(super) embedder_id: Arc<str>,
    pub(super) scope: MemoryScope,
    pub(super) id: MemoryId,
    pub(super) source_digest: Digest,
    pub(super) vector: EmbeddingVector,
}

/// [`MemoryStore::store_embedding`]: dimension and space-capacity checks,
/// then the staleness guard, then an `INSERT OR REPLACE` of the
/// unit-normalized little-endian f32 BLOB — all in one transaction.
///
/// [`MemoryStore::store_embedding`]: super::super::MemoryStore::store_embedding
pub(super) fn store_embedding(
    connection: &mut Connection,
    write: &EmbeddingWrite,
    now: Timestamp,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    validate_embedder_id(&write.embedder_id)?;
    validate_scope(&write.scope)?;
    if write.vector.dimensions() > limits.max_embedding_dimensions {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_exceeded",
        });
    }
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| sqlite_unavailable())?;
    match space_dimensions(&transaction, &write.embedder_id)? {
        // The first vector stored in a space fixes its dimensionality.
        Some(dimensions) if dimensions != write.vector.dimensions() => {
            return Err(MemoryStoreError::InvalidRequest {
                reason: "memory_embedding_dimensions_mismatch",
            });
        }
        Some(_) => {}
        None => reserve_embedding_space(&transaction, limits)?,
    }
    // Staleness guard: only a live record whose current source digest still
    // matches takes the write; anything else is a silent no-op and the
    // anti-join re-surfaces the record.
    let current = fetch_record(&transaction, &write.scope, &write.id, now)?
        .filter(|record| !record.tombstoned && record.superseded_by.is_none());
    let Some(record) = current else {
        return transaction.commit().map_err(|_| sqlite_unavailable());
    };
    if embedding_source_digest(&embedding_source_text(&record))? != write.source_digest {
        return transaction.commit().map_err(|_| sqlite_unavailable());
    }
    let unit_vector = write.vector.unit_normalized();
    transaction
        .execute(
            "INSERT OR REPLACE INTO memory_embeddings
             (scope_digest, id, embedder_id, dimensions, source_digest, vector, embedded_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
            params![
                scope_key(&write.scope)?,
                write.id.as_str(),
                write.embedder_id.as_ref(),
                i64::try_from(unit_vector.dimensions()).unwrap_or(i64::MAX),
                write.source_digest.to_hex(),
                vector_to_blob(&unit_vector),
                now.as_unix_ms(),
            ],
        )
        .map_err(|_| sqlite_unavailable())?;
    transaction.commit().map_err(|_| sqlite_unavailable())
}

/// [`MemoryStore::forget_embedding_space`]: drop every row of one space
/// (embedder rotation); its records become pending again.
///
/// [`MemoryStore::forget_embedding_space`]: super::super::MemoryStore::forget_embedding_space
pub(super) fn forget_embedding_space(
    connection: &Connection,
    embedder_id: &str,
) -> Result<(), MemoryStoreError> {
    validate_embedder_id(embedder_id)?;
    connection
        .execute(
            "DELETE FROM memory_embeddings WHERE embedder_id = ?1",
            params![embedder_id],
        )
        .map_err(|_| sqlite_unavailable())?;
    Ok(())
}

/// Dimensionality of the space `embedder_id`, fixed by its first stored row
/// (every row in a space carries equal dimensions by construction); `None`
/// when the space holds no rows.
fn space_dimensions(
    connection: &Connection,
    embedder_id: &str,
) -> Result<Option<usize>, MemoryStoreError> {
    let dimensions: Option<i64> = connection
        .query_row(
            "SELECT dimensions FROM memory_embeddings WHERE embedder_id = ?1 LIMIT 1",
            params![embedder_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| sqlite_unavailable())?;
    Ok(dimensions.map(|value| usize::try_from(value).unwrap_or(usize::MAX)))
}

/// Fail closed when creating one more space would exceed the cap. Spaces
/// are derived from live rows, so dropping a space frees its slot.
fn reserve_embedding_space(
    transaction: &Transaction<'_>,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let spaces: i64 = transaction
        .query_row(
            "SELECT COUNT(DISTINCT embedder_id) FROM memory_embeddings",
            [],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    if usize::try_from(spaces).unwrap_or(usize::MAX) >= limits.max_embedding_spaces {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "embedding_spaces",
            limit: u64::try_from(limits.max_embedding_spaces).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

/// Encode a vector as the little-endian f32 BLOB the schema stores.
fn vector_to_blob(vector: &EmbeddingVector) -> Vec<u8> {
    let mut blob = Vec::with_capacity(vector.dimensions().saturating_mul(4));
    for component in vector.as_slice() {
        blob.extend_from_slice(&component.to_le_bytes());
    }
    blob
}

fn reserve_receipt(
    transaction: &Transaction<'_>,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let count: i64 = transaction
        .query_row("SELECT COUNT(*) FROM memory_idempotency", [], |row| {
            row.get(0)
        })
        .map_err(|_| sqlite_unavailable())?;
    if usize::try_from(count).unwrap_or(usize::MAX) >= limits.max_idempotency_keys {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "idempotency_keys",
            limit: u64::try_from(limits.max_idempotency_keys).unwrap_or(u64::MAX),
        });
    }
    Ok(())
}

fn reserve_record(
    transaction: &Transaction<'_>,
    incoming: &MemoryRecord,
    limits: MemoryStoreLimits,
) -> Result<(), MemoryStoreError> {
    let existing_inline: Option<Option<i64>> = transaction
        .query_row(
            "SELECT length(CAST(body_inline AS BLOB)) FROM memory_records
             WHERE scope_digest = ?1 AND id = ?2",
            params![scope_key(&incoming.scope)?, incoming.id.as_str()],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| sqlite_unavailable())?;
    if existing_inline.is_none() {
        let count: i64 = transaction
            .query_row("SELECT COUNT(*) FROM memory_records", [], |row| row.get(0))
            .map_err(|_| sqlite_unavailable())?;
        if usize::try_from(count).unwrap_or(usize::MAX) >= limits.max_records {
            return Err(MemoryStoreError::CapacityExceeded {
                resource: "records",
                limit: u64::try_from(limits.max_records).unwrap_or(u64::MAX),
            });
        }
    }
    let total: i64 = transaction
        .query_row(
            "SELECT COALESCE(SUM(length(CAST(body_inline AS BLOB))), 0) FROM memory_records",
            [],
            |row| row.get(0),
        )
        .map_err(|_| sqlite_unavailable())?;
    let old_bytes = existing_inline.flatten().unwrap_or(0);
    let incoming_bytes = match &incoming.body {
        MemoryBody::Inline(body) => i64::try_from(body.len()).unwrap_or(i64::MAX),
        MemoryBody::Blob { .. } => 0,
    };
    let next = total
        .saturating_sub(old_bytes)
        .checked_add(incoming_bytes)
        .ok_or(MemoryStoreError::CapacityExceeded {
            resource: "inline_bytes",
            limit: limits.max_inline_bytes,
        })?;
    if u64::try_from(next).unwrap_or(u64::MAX) > limits.max_inline_bytes {
        return Err(MemoryStoreError::CapacityExceeded {
            resource: "inline_bytes",
            limit: limits.max_inline_bytes,
        });
    }
    Ok(())
}

/// [`MemoryQuery::ExactId`] search: a single primary-key lookup.
fn search_exact_id(
    connection: &Connection,
    scope: &MemoryScope,
    id: &MemoryId,
    now: Timestamp,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let record = connection
        .query_row(
            "SELECT * FROM memory_records
             WHERE id = ?1 AND scope_digest = ?2
               AND tombstoned = 0 AND superseded_by IS NULL
               AND (expires_at IS NULL OR expires_at > ?3)",
            params![id.as_str(), scope_key(scope)?, now.as_unix_ms()],
            record_from_row,
        )
        .optional()
        .map_err(|_| sqlite_unavailable())?;
    Ok(record
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
    limit: usize,
    now: Timestamp,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let folded = keywords
        .iter()
        .map(|keyword| keyword.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let requested_json = serde_json::to_string(&folded).map_err(|_| sqlite_unavailable())?;
    let mut statement = connection
        .prepare(
            "WITH requested(keyword) AS (
               SELECT CAST(value AS TEXT) FROM json_each(?2)
             )
             SELECT r.*, COUNT(DISTINCT requested.keyword) AS matched_count,
                    MIN(k.keyword) AS first_keyword
             FROM memory_keywords k
             JOIN requested ON requested.keyword = k.keyword_folded
             JOIN memory_records r
               ON r.scope_digest = k.scope_digest AND r.id = k.id
             WHERE r.scope_digest = ?1
               AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND (r.expires_at IS NULL OR r.expires_at > ?4)
             GROUP BY r.scope_digest, r.id
             ORDER BY matched_count DESC, r.id
             LIMIT ?3",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(
            params![
                scope_key(scope)?,
                requested_json,
                i64::try_from(limit).unwrap_or(i64::MAX),
                now.as_unix_ms(),
            ],
            |row| {
                let record = record_from_row(row)?;
                let matched_count: i64 = row.get("matched_count")?;
                let keyword: String = row.get("first_keyword")?;
                Ok((record, matched_count, keyword))
            },
        )
        .map_err(|_| sqlite_unavailable())?;
    let mut hits = Vec::new();
    for row in rows {
        let (record, matched_count, keyword) = row.map_err(|_| sqlite_unavailable())?;
        hits.push(MemoryHit {
            record,
            score: u32::try_from(matched_count).unwrap_or(u32::MAX),
            matched: MatchEvidence::Keyword(Arc::from(keyword)),
        });
    }
    Ok(hits)
}

/// [`MemoryQuery::FullText`] search: scope-filtered FTS5 `bm25` ranking.
/// `bm25` scores are floats; they are never stored, only used to order the
/// SQL result set, which is then mapped to a descending rank-order `u32`.
fn search_full_text(
    connection: &Connection,
    scope: &MemoryScope,
    text: &str,
    limit: usize,
    now: Timestamp,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let sanitized = sanitize_fts_query(text);
    if sanitized.is_empty() {
        return Ok(Vec::new());
    }
    let mut statement = connection
        .prepare(
            "SELECT r.* FROM memory_fts f
             JOIN memory_records r
               ON r.scope_digest = f.scope_digest AND r.id = f.id
             WHERE memory_fts MATCH ?1
               AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND r.scope_digest = ?2
               AND (r.expires_at IS NULL OR r.expires_at > ?4)
             ORDER BY bm25(memory_fts)
             LIMIT ?3",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(
            params![
                sanitized,
                scope_key(scope)?,
                i64::try_from(limit).unwrap_or(i64::MAX),
                now.as_unix_ms(),
            ],
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

/// [`MemoryQuery::Embedding`] search: brute-force exact ranking over the
/// scope's live rows in the space — the dot product of unit-normalized
/// vectors mapped through the shared [`similarity_score`], so the ranking is
/// byte-identical with [`super::super::InProcessMemoryStore`]'s. Ordering
/// and the caller's limit are applied by [`sqlite_search`]'s shared sort.
fn search_embedding(
    connection: &Connection,
    scope: &MemoryScope,
    embedder_id: &str,
    vector: &EmbeddingVector,
    now: Timestamp,
) -> Result<Vec<MemoryHit>, MemoryStoreError> {
    let Some(dimensions) = space_dimensions(connection, embedder_id)? else {
        // A space no embedder ever populated holds nothing.
        return Ok(Vec::new());
    };
    if dimensions != vector.dimensions() {
        return Err(MemoryStoreError::InvalidRequest {
            reason: "memory_embedding_dimensions_mismatch",
        });
    }
    let query = vector.unit_normalized();
    let mut statement = connection
        .prepare(
            "SELECT r.*, e.vector AS embedding_blob
             FROM memory_embeddings e
             JOIN memory_records r
               ON r.scope_digest = e.scope_digest AND r.id = e.id
             WHERE e.embedder_id = ?1 AND e.scope_digest = ?2
               AND r.tombstoned = 0 AND r.superseded_by IS NULL
               AND (r.expires_at IS NULL OR r.expires_at > ?3)",
        )
        .map_err(|_| sqlite_unavailable())?;
    let rows = statement
        .query_map(
            params![embedder_id, scope_key(scope)?, now.as_unix_ms()],
            |row| {
                let record = record_from_row(row)?;
                let blob: Vec<u8> = row.get("embedding_blob")?;
                Ok((record, blob))
            },
        )
        .map_err(|_| sqlite_unavailable())?;
    let mut hits = Vec::new();
    for row in rows {
        let (record, blob) = row.map_err(|_| sqlite_unavailable())?;
        // Skip an undecodable embedding BLOB so one bad row does not fail search.
        let Some(stored) = vector_from_blob(&blob) else {
            continue;
        };
        let Some(dot) = query.dot(&stored) else {
            continue;
        };
        hits.push(MemoryHit {
            record,
            score: similarity_score(dot),
            matched: MatchEvidence::Semantic,
        });
    }
    Ok(hits)
}

/// Decode a stored little-endian f32 BLOB back into a vector; `None` when
/// the BLOB is not a valid embedding vector.
fn vector_from_blob(blob: &[u8]) -> Option<EmbeddingVector> {
    if blob.is_empty() || !blob.len().is_multiple_of(4) {
        return None;
    }
    let mut components = Vec::with_capacity(blob.len() / 4);
    for chunk in blob.chunks_exact(4) {
        components.push(f32::from_le_bytes(chunk.try_into().ok()?));
    }
    EmbeddingVector::try_new(components).ok()
}
