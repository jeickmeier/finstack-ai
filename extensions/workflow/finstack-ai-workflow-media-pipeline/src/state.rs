//! Render state and its adapter-owned stores.
//!
//! [`RenderState`] is the journal-independent progress record for one movie
//! render: one row per `(tenant_scope, render_id)`, versioned by an
//! optimistic-concurrency `revision`. Stores live in the same file as the
//! journal but are not part of the journal schema (mirrors
//! `finstack-ai-workflow-local`'s cron adapter table).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use finstack_ai_kernel::{ArtifactRef, Digest};
use rusqlite::{Connection, ErrorCode, OptionalExtension, TransactionBehavior, params};
use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Per-scene progress stage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SceneStage {
    /// Waiting on the start-frame image to be generated or resolved.
    PendingStartFrame,
    /// Waiting on the end-frame image to be generated or resolved.
    PendingEndFrame,
    /// Waiting to submit the video generation job.
    PendingSubmit,
    /// Job submitted; waiting on the provider to finish.
    Polling,
    /// Job finished; waiting to download the resulting clip.
    PendingDownload,
    /// Scene finished successfully.
    Done,
    /// Scene failed and will not be retried automatically.
    Failed,
}

/// Progress and artifacts for a single scene within a render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SceneState {
    /// Scene identifier, matching the plan's `SceneSpec::id`.
    pub scene_id: String,
    /// Current stage in the scene's generation lifecycle.
    pub stage: SceneStage,
    /// Resolved start-frame artifact, once generated or fetched.
    pub start_frame_artifact: Option<ArtifactRef>,
    /// Resolved start-frame URL, when the frame source was a URL.
    pub start_frame_url: Option<String>,
    /// Resolved end-frame artifact, once generated or fetched.
    pub end_frame_artifact: Option<ArtifactRef>,
    /// Resolved end-frame URL, when the frame source was a URL.
    pub end_frame_url: Option<String>,
    /// Provider job identifier for the submitted video generation job.
    pub job_id: Option<String>,
    /// Downloaded video clip artifact, once the job finishes.
    pub clip_artifact: Option<ArtifactRef>,
    /// Human-readable failure reason, when `stage` is [`SceneStage::Failed`].
    pub failure: Option<String>,
    /// Whether this scene has already been resubmitted once after a failure.
    pub resubmitted: bool,
}

/// Overall render progress.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RenderStatus {
    /// Scenes are still being generated.
    Running,
    /// All scenes are done; the final composition is in progress.
    Composing,
    /// The render finished successfully.
    Completed,
    /// The render failed and will not be retried automatically.
    Failed,
}

/// Durable, tenant-scoped progress record for one movie render.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RenderState {
    /// Tenant that owns this render.
    pub tenant_scope: Arc<str>,
    /// Deterministic render identity: `"render-"` + first 24 hex characters
    /// of `plan_digest`. Resubmitting the same plan resumes the same render.
    pub render_id: Arc<str>,
    /// Original plan JSON, kept verbatim for replay and audit.
    pub plan_json: String,
    /// Digest of `plan_json`, used to derive `render_id`.
    pub plan_digest: Digest,
    /// Overall render progress.
    pub status: RenderStatus,
    /// Per-scene progress, in plan order.
    pub scenes: Vec<SceneState>,
    /// Final composed video artifact, once the render completes.
    pub final_artifact: Option<ArtifactRef>,
    /// Composed SRT transcript artifact, when captions are requested.
    pub transcript_srt_artifact: Option<ArtifactRef>,
    /// Composed WebVTT transcript artifact, when captions are requested.
    pub transcript_vtt_artifact: Option<ArtifactRef>,
    /// Optimistic-concurrency revision. Starts at `0`; `update` persists
    /// `revision + 1` on success.
    pub revision: u64,
}

/// Adapter-owned store for [`RenderState`] rows. Not part of the journal
/// schema.
pub trait RenderStateStore: Send + Sync {
    /// Insert a brand-new row.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Integrity`] when a row already exists for
    /// `(state.tenant_scope, state.render_id)`, or [`StateError::Unavailable`]
    /// when the store cannot be reached.
    fn insert(&self, state: &RenderState) -> Result<(), StateError>;

    /// Load one row, scoped to `tenant_scope`.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Unavailable`] or [`StateError::Integrity`] when
    /// the stored row cannot be read back.
    fn load(&self, tenant_scope: &str, render_id: &str) -> Result<Option<RenderState>, StateError>;

    /// Compare-and-set update.
    ///
    /// Wins only when the stored revision still equals `state.revision`;
    /// persists `state.revision + 1` on success. Returns `false` (not an
    /// error) when the caller lost the race to a concurrent writer.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Unavailable`] or [`StateError::Integrity`] when
    /// the store cannot be reached or the update cannot be persisted.
    fn update(&self, state: &RenderState) -> Result<bool, StateError>;
}

/// Adapter-owned failures for [`RenderStateStore`]. Not kernel record errors.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum StateError {
    /// The store is temporarily unreachable.
    #[error("render state store unavailable: {code}")]
    Unavailable {
        /// Stable reason code.
        code: &'static str,
    },
    /// A stored row could not be decoded, or a write would violate an
    /// invariant the store enforces.
    #[error("render state store integrity: {code}")]
    Integrity {
        /// Stable reason code.
        code: &'static str,
    },
}

impl StateError {
    /// Stable lowercase error code.
    #[must_use]
    pub const fn code(&self) -> &'static str {
        match self {
            Self::Unavailable { code } | Self::Integrity { code } => code,
        }
    }
}

type MemoryRows = BTreeMap<(Arc<str>, Arc<str>), RenderState>;

/// In-process table, keyed by `(tenant_scope, render_id)`.
#[derive(Debug, Default)]
pub struct MemoryRenderStateStore {
    inner: Mutex<MemoryRows>,
}

impl MemoryRenderStateStore {
    /// Empty adapter table.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }
}

impl RenderStateStore for MemoryRenderStateStore {
    fn insert(&self, state: &RenderState) -> Result<(), StateError> {
        let mut inner = self.inner.lock().map_err(|_| StateError::Unavailable {
            code: "memory_state_lock_poisoned",
        })?;
        let key = (Arc::clone(&state.tenant_scope), Arc::clone(&state.render_id));
        if inner.contains_key(&key) {
            return Err(StateError::Integrity {
                code: "render_exists",
            });
        }
        inner.insert(key, state.clone());
        Ok(())
    }

    fn load(&self, tenant_scope: &str, render_id: &str) -> Result<Option<RenderState>, StateError> {
        let inner = self.inner.lock().map_err(|_| StateError::Unavailable {
            code: "memory_state_lock_poisoned",
        })?;
        Ok(inner
            .get(&(Arc::from(tenant_scope), Arc::from(render_id)))
            .cloned())
    }

    fn update(&self, state: &RenderState) -> Result<bool, StateError> {
        let mut inner = self.inner.lock().map_err(|_| StateError::Unavailable {
            code: "memory_state_lock_poisoned",
        })?;
        let key = (Arc::clone(&state.tenant_scope), Arc::clone(&state.render_id));
        let Some(row) = inner.get_mut(&key) else {
            return Ok(false);
        };
        if row.revision != state.revision {
            return Ok(false);
        }
        let mut next = state.clone();
        next.revision = state.revision.checked_add(1).ok_or(StateError::Integrity {
            code: "revision_overflow",
        })?;
        *row = next;
        Ok(true)
    }
}

const STATE_DDL: &str = "
CREATE TABLE IF NOT EXISTS finstack_workflow_media_pipeline (
  tenant_scope TEXT NOT NULL,
  render_id TEXT NOT NULL,
  revision INTEGER NOT NULL,
  state_json TEXT NOT NULL,
  PRIMARY KEY (tenant_scope, render_id)
);
";

/// Sqlite table in the same file as the journal store.
///
/// Rows are adapter state: they do not change `PRAGMA user_version` and are
/// not kernel records.
pub struct SqliteRenderStateStore {
    conn: Mutex<Connection>,
}

impl SqliteRenderStateStore {
    /// Open or create the adapter table in `path`.
    ///
    /// # Errors
    ///
    /// Returns [`StateError::Unavailable`] when the file cannot be opened or
    /// the table cannot be created.
    pub fn open(path: impl AsRef<Path>) -> Result<Self, StateError> {
        let path: PathBuf = path.as_ref().to_path_buf();
        let conn = Connection::open(&path).map_err(|_| StateError::Unavailable {
            code: "sqlite_state_open",
        })?;
        conn.busy_timeout(Duration::from_secs(1))
            .map_err(|_| StateError::Unavailable {
                code: "sqlite_state_busy_timeout",
            })?;
        if !is_memory_path(&path) {
            conn.pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| StateError::Unavailable {
                    code: "sqlite_state_wal",
                })?;
        }
        conn.execute_batch(STATE_DDL)
            .map_err(|_| StateError::Unavailable {
                code: "sqlite_state_schema",
            })?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    fn with_conn<T>(
        &self,
        body: impl FnOnce(&Connection) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let conn = self.conn.lock().map_err(|_| StateError::Unavailable {
            code: "sqlite_state_lock_poisoned",
        })?;
        body(&conn)
    }

    fn with_conn_mut<T>(
        &self,
        body: impl FnOnce(&mut Connection) -> Result<T, StateError>,
    ) -> Result<T, StateError> {
        let mut conn = self.conn.lock().map_err(|_| StateError::Unavailable {
            code: "sqlite_state_lock_poisoned",
        })?;
        body(&mut conn)
    }
}

impl RenderStateStore for SqliteRenderStateStore {
    fn insert(&self, state: &RenderState) -> Result<(), StateError> {
        let json = serde_json::to_string(state).map_err(|_| StateError::Integrity {
            code: "state_json_encode",
        })?;
        let revision = i64_from_u64(state.revision)?;
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO finstack_workflow_media_pipeline (
                    tenant_scope, render_id, revision, state_json
                 ) VALUES (?1, ?2, ?3, ?4)",
                params![
                    state.tenant_scope.as_ref(),
                    state.render_id.as_ref(),
                    revision,
                    json,
                ],
            )
            .map_err(|error| {
                if is_constraint_violation(&error) {
                    StateError::Integrity {
                        code: "render_exists",
                    }
                } else {
                    StateError::Unavailable {
                        code: "sqlite_state_insert",
                    }
                }
            })?;
            Ok(())
        })
    }

    fn load(&self, tenant_scope: &str, render_id: &str) -> Result<Option<RenderState>, StateError> {
        self.with_conn(|conn| {
            let row: Option<String> = conn
                .query_row(
                    "SELECT state_json FROM finstack_workflow_media_pipeline
                     WHERE tenant_scope = ?1 AND render_id = ?2",
                    params![tenant_scope, render_id],
                    |row| row.get(0),
                )
                .optional()
                .map_err(|_| StateError::Unavailable {
                    code: "sqlite_state_query",
                })?;
            row.map(|json| {
                serde_json::from_str(&json).map_err(|_| StateError::Integrity {
                    code: "state_json_invalid",
                })
            })
            .transpose()
        })
    }

    fn update(&self, state: &RenderState) -> Result<bool, StateError> {
        let mut next = state.clone();
        next.revision = state.revision.checked_add(1).ok_or(StateError::Integrity {
            code: "revision_overflow",
        })?;
        let json = serde_json::to_string(&next).map_err(|_| StateError::Integrity {
            code: "state_json_encode",
        })?;
        let expected_revision = i64_from_u64(state.revision)?;
        self.with_conn_mut(|conn| {
            let tx = conn
                .transaction_with_behavior(TransactionBehavior::Immediate)
                .map_err(|_| StateError::Unavailable {
                    code: "sqlite_state_begin_immediate",
                })?;
            tx.execute(
                "UPDATE finstack_workflow_media_pipeline
                 SET revision = revision + 1, state_json = ?1
                 WHERE tenant_scope = ?2 AND render_id = ?3 AND revision = ?4",
                params![
                    json,
                    state.tenant_scope.as_ref(),
                    state.render_id.as_ref(),
                    expected_revision,
                ],
            )
            .map_err(|_| StateError::Unavailable {
                code: "sqlite_state_update",
            })?;
            let won = tx.changes() == 1;
            tx.commit().map_err(|_| StateError::Unavailable {
                code: "sqlite_state_commit",
            })?;
            Ok(won)
        })
    }
}

fn is_constraint_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(ffi_err, _) if ffi_err.code == ErrorCode::ConstraintViolation
    )
}

fn is_memory_path(path: &Path) -> bool {
    let text = path.to_string_lossy();
    text == ":memory:" || text.contains("mode=memory")
}

fn i64_from_u64(value: u64) -> Result<i64, StateError> {
    i64::try_from(value).map_err(|_| StateError::Integrity {
        code: "revision_out_of_range",
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_state() -> RenderState {
        let bytes = std::fs::read(
            finstack_ai_test::repo_root()
                .join("fixtures/compatibility/movie-plan/valid-two-scene.json"),
        )
        .expect("fixture");
        let plan_json = String::from_utf8(bytes).expect("utf8");
        let plan_digest = Digest::raw_json(plan_json.as_bytes());
        let render_id = format!("render-{}", &plan_digest.to_hex()[..24]);
        RenderState {
            tenant_scope: Arc::from("tenant-a"),
            render_id: Arc::from(render_id),
            plan_json,
            plan_digest,
            status: RenderStatus::Running,
            scenes: vec![SceneState {
                scene_id: "scene-01".to_string(),
                stage: SceneStage::PendingStartFrame,
                start_frame_artifact: None,
                start_frame_url: None,
                end_frame_artifact: None,
                end_frame_url: None,
                job_id: None,
                clip_artifact: None,
                failure: None,
                resubmitted: false,
            }],
            final_artifact: None,
            transcript_srt_artifact: None,
            transcript_vtt_artifact: None,
            revision: 0,
        }
    }

    #[test]
    fn sqlite_store_round_trips_and_cas_guards_updates() {
        let dir = tempfile::tempdir().expect("dir");
        let store = SqliteRenderStateStore::open(dir.path().join("journal.sqlite")).expect("open");
        let mut state = sample_state();
        store.insert(&state).expect("insert");
        assert_eq!(
            store.insert(&state).expect_err("duplicate render_id").code(),
            "render_exists"
        );
        assert_eq!(
            store
                .load("tenant-a", state.render_id.as_ref())
                .expect("load")
                .expect("row")
                .revision,
            0
        );
        state.status = RenderStatus::Composing;
        assert!(store.update(&state).expect("update"), "first CAS wins");
        assert!(!store.update(&state).expect("update"), "stale revision loses");
        let reloaded = store
            .load("tenant-a", state.render_id.as_ref())
            .expect("load")
            .expect("row");
        assert_eq!(reloaded.revision, 1);
        assert!(matches!(reloaded.status, RenderStatus::Composing));
        assert!(
            store
                .load("tenant-b", state.render_id.as_ref())
                .expect("load")
                .is_none()
        );
    }

    #[test]
    fn memory_store_matches_sqlite_semantics() {
        let store = MemoryRenderStateStore::new();
        let mut state = sample_state();
        store.insert(&state).expect("insert");
        assert!(store.insert(&state).is_err(), "duplicate render_id");
        assert_eq!(
            store
                .load("tenant-a", state.render_id.as_ref())
                .expect("load")
                .expect("row")
                .revision,
            0
        );
        state.status = RenderStatus::Composing;
        assert!(store.update(&state).expect("update"), "first CAS wins");
        assert!(!store.update(&state).expect("update"), "stale revision loses");
        let reloaded = store
            .load("tenant-a", state.render_id.as_ref())
            .expect("load")
            .expect("row");
        assert_eq!(reloaded.revision, 1);
        assert!(matches!(reloaded.status, RenderStatus::Composing));
        assert!(
            store
                .load("tenant-b", state.render_id.as_ref())
                .expect("load")
                .is_none()
        );
    }

    #[test]
    fn two_sqlite_handles_share_one_table() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("journal.sqlite");
        let first = SqliteRenderStateStore::open(&path).expect("first");
        let second = SqliteRenderStateStore::open(&path).expect("second");
        let mut state = sample_state();
        first.insert(&state).expect("insert via first");
        state.status = RenderStatus::Composing;
        assert!(
            second.update(&state).expect("update via second"),
            "second handle wins CAS"
        );
        let reloaded = first
            .load("tenant-a", state.render_id.as_ref())
            .expect("load via first")
            .expect("row");
        assert_eq!(reloaded.revision, 1);
        assert!(matches!(reloaded.status, RenderStatus::Composing));
    }

    #[test]
    fn sqlite_insert_reports_unavailable_not_render_exists_on_genuine_failure() {
        let dir = tempfile::tempdir().expect("dir");
        let path = dir.path().join("journal.sqlite");
        let store = SqliteRenderStateStore::open(&path).expect("open");

        // Sabotage the table out from under the store via a second raw
        // connection, so the next insert fails for a reason that is NOT a
        // constraint violation (table missing, not a duplicate row).
        let saboteur = Connection::open(&path).expect("saboteur connection");
        saboteur
            .execute_batch("DROP TABLE finstack_workflow_media_pipeline;")
            .expect("drop table");
        drop(saboteur);

        let mut state = sample_state();
        state.render_id = Arc::from("render-does-not-exist-yet");
        let error = store.insert(&state).expect_err("insert must fail");
        assert_eq!(
            error.code(),
            "sqlite_state_insert",
            "a missing table must report Unavailable, not be mistaken for a duplicate row"
        );
        assert!(matches!(error, StateError::Unavailable { .. }));
    }
}
