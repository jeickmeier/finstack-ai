//! Dedicated `SQLite` worker thread and [`SqliteMemoryStore`] port impl.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

use finstack_ai_embeddings::vector::EmbeddingVector;
use finstack_ai_kernel::Digest;

use crate::record::{MemoryClock, MemoryId, MemoryRecord, MemoryScope, system_clock};
use finstack_ai_runtime::ports::PortFuture;

use super::super::{
    EmbeddingCoverage, EmbeddingSource, MemoryArtifactAction, MemoryHit, MemoryListing, MemoryPage,
    MemoryQuery, MemoryStore, MemoryStoreDescriptor, MemoryStoreError, MemoryStoreLimits,
    PutOutcome,
};
use super::queries::{
    EmbeddingWrite, acknowledge_artifact_action, forget_embedding_space, pending_artifact_actions,
    pending_embedding_sources, sqlite_correct, sqlite_forget, sqlite_get, sqlite_list, sqlite_put,
    sqlite_search, sqlite_unavailable, store_embedding,
};
use super::schema::{apply_schema, ensure_store_identity, ensure_v2_auxiliary_tables};

/// Lock-wait applied to every opened connection before `SQLITE_BUSY`.
const BUSY_TIMEOUT: Duration = Duration::from_secs(1);

/// Maximum number of `SQLite` operations awaiting the dedicated worker.
const WORK_QUEUE_CAPACITY: usize = 64;

/// Durable [`MemoryStore`] backed by a single `SQLite` connection with an
/// FTS5 full-text index.
///
/// Available only on native targets with the `sqlite` feature enabled.
#[derive(Debug)]
pub struct SqliteMemoryStore {
    sender: Option<mpsc::Sender<Command>>,
    worker: Option<std::thread::JoinHandle<()>>,
    store_id: Arc<str>,
    limits: MemoryStoreLimits,
}

impl Drop for SqliteMemoryStore {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

impl MemoryStore for SqliteMemoryStore {
    fn descriptor(&self) -> MemoryStoreDescriptor {
        MemoryStoreDescriptor {
            store_id: Arc::clone(&self.store_id),
            durable: true,
            manages_artifact_ownership: true,
            limits: self.limits,
        }
    }

    fn put(
        &self,
        key: Arc<str>,
        record: MemoryRecord,
    ) -> PortFuture<Result<PutOutcome, MemoryStoreError>> {
        self.dispatch(move |reply| Command::Put { key, record, reply })
    }

    fn get(
        &self,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<Option<MemoryRecord>, MemoryStoreError>> {
        self.dispatch(move |reply| Command::Get { scope, id, reply })
    }

    fn search(
        &self,
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryHit>, MemoryStoreError>> {
        self.dispatch(move |reply| Command::Search {
            scope,
            query,
            limit,
            reply,
        })
    }

    fn forget(
        &self,
        key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.dispatch(move |reply| Command::Forget {
            key,
            scope,
            id,
            reply,
        })
    }

    fn correct(
        &self,
        key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.dispatch(move |reply| Command::Correct {
            key,
            scope,
            old,
            replacement,
            reply,
        })
    }

    fn list(
        &self,
        scope: MemoryScope,
        page: MemoryPage,
    ) -> PortFuture<Result<MemoryListing, MemoryStoreError>> {
        self.dispatch(move |reply| Command::List { scope, page, reply })
    }

    fn pending_artifact_actions(
        &self,
        limit: usize,
    ) -> PortFuture<Result<Vec<MemoryArtifactAction>, MemoryStoreError>> {
        self.dispatch(move |reply| Command::PendingArtifactActions { limit, reply })
    }

    fn acknowledge_artifact_action(
        &self,
        action_id: Digest,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.dispatch(move |reply| Command::AcknowledgeArtifactAction { action_id, reply })
    }

    fn embedding_coverage(
        &self,
        scope: MemoryScope,
        embedder_id: Arc<str>,
    ) -> PortFuture<Result<Option<EmbeddingCoverage>, MemoryStoreError>> {
        self.dispatch(move |reply| Command::EmbeddingCoverage {
            scope,
            embedder_id,
            reply,
        })
    }

    fn pending_embedding_sources(
        &self,
        embedder_id: Arc<str>,
        limit: usize,
    ) -> PortFuture<Result<Vec<EmbeddingSource>, MemoryStoreError>> {
        self.dispatch(move |reply| Command::PendingEmbeddingSources {
            embedder_id,
            limit,
            reply,
        })
    }

    fn store_embedding(
        &self,
        embedder_id: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
        source_digest: Digest,
        vector: EmbeddingVector,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        let write = EmbeddingWrite {
            embedder_id,
            scope,
            id,
            source_digest,
            vector,
        };
        self.dispatch(move |reply| Command::StoreEmbedding { write, reply })
    }

    fn forget_embedding_space(
        &self,
        embedder_id: Arc<str>,
    ) -> PortFuture<Result<(), MemoryStoreError>> {
        self.dispatch(move |reply| Command::ForgetEmbeddingSpace { embedder_id, reply })
    }
}

enum Command {
    Put {
        key: Arc<str>,
        record: MemoryRecord,
        reply: oneshot::Sender<Result<PutOutcome, MemoryStoreError>>,
    },
    Get {
        scope: MemoryScope,
        id: MemoryId,
        reply: oneshot::Sender<Result<Option<MemoryRecord>, MemoryStoreError>>,
    },
    Search {
        scope: MemoryScope,
        query: MemoryQuery,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<MemoryHit>, MemoryStoreError>>,
    },
    Forget {
        key: Arc<str>,
        scope: MemoryScope,
        id: MemoryId,
        reply: oneshot::Sender<Result<(), MemoryStoreError>>,
    },
    Correct {
        key: Arc<str>,
        scope: MemoryScope,
        old: MemoryId,
        replacement: MemoryRecord,
        reply: oneshot::Sender<Result<(), MemoryStoreError>>,
    },
    List {
        scope: MemoryScope,
        page: MemoryPage,
        reply: oneshot::Sender<Result<MemoryListing, MemoryStoreError>>,
    },
    PendingArtifactActions {
        limit: usize,
        reply: oneshot::Sender<Result<Vec<MemoryArtifactAction>, MemoryStoreError>>,
    },
    AcknowledgeArtifactAction {
        action_id: Digest,
        reply: oneshot::Sender<Result<(), MemoryStoreError>>,
    },
    EmbeddingCoverage {
        scope: MemoryScope,
        embedder_id: Arc<str>,
        reply: oneshot::Sender<Result<Option<EmbeddingCoverage>, MemoryStoreError>>,
    },
    PendingEmbeddingSources {
        embedder_id: Arc<str>,
        limit: usize,
        reply: oneshot::Sender<Result<Vec<EmbeddingSource>, MemoryStoreError>>,
    },
    StoreEmbedding {
        write: EmbeddingWrite,
        reply: oneshot::Sender<Result<(), MemoryStoreError>>,
    },
    ForgetEmbeddingSpace {
        embedder_id: Arc<str>,
        reply: oneshot::Sender<Result<(), MemoryStoreError>>,
    },
}

impl SqliteMemoryStore {
    /// Open (creating if absent) a file-backed store at `path`.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] if the connection cannot be
    /// opened, pragmas cannot be applied, or the schema is missing/mismatched.
    pub fn try_open(path: &Path) -> Result<Self, MemoryStoreError> {
        let connection = Connection::open(path).map_err(|_| sqlite_unavailable())?;
        let canonical_path = std::fs::canonicalize(path).map_err(|_| sqlite_unavailable())?;
        let path_digest = Digest::domain_separated(
            "memory-sqlite-path",
            1,
            canonical_path.to_string_lossy().as_bytes(),
        )
        .map_err(|_| sqlite_unavailable())?;
        let store_id = Arc::from(format!("memory.sqlite-v2.{}", path_digest.to_hex()));
        Self::from_connection(
            connection,
            false,
            system_clock(),
            MemoryStoreLimits::default(),
            &store_id,
        )
    }

    /// Open a private in-memory store (not shared across connections).
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] if the connection cannot be
    /// opened, pragmas cannot be applied, or the schema is missing/mismatched.
    pub fn try_open_in_memory() -> Result<Self, MemoryStoreError> {
        let connection = Connection::open_in_memory().map_err(|_| sqlite_unavailable())?;
        Self::from_connection(
            connection,
            true,
            system_clock(),
            MemoryStoreLimits::default(),
            "memory.sqlite-v2.in-memory",
        )
    }

    /// Open an in-memory store with deterministic time and explicit limits.
    ///
    /// Intended for embedders that supply their own clock and for tests.
    ///
    /// # Errors
    ///
    /// Returns [`MemoryStoreError::Unavailable`] if the connection cannot be
    /// opened, configured, or migrated.
    pub fn try_open_in_memory_with(
        clock: MemoryClock,
        limits: MemoryStoreLimits,
    ) -> Result<Self, MemoryStoreError> {
        let connection = Connection::open_in_memory().map_err(|_| sqlite_unavailable())?;
        Self::from_connection(
            connection,
            true,
            clock,
            limits,
            "memory.sqlite-v2.in-memory",
        )
    }

    /// Queue one command for the worker and await its reply. Every store
    /// operation funnels through here, so a dropped sender (store closed)
    /// or a dead worker surfaces as the same stable `Unavailable` error.
    fn dispatch<T: Send + 'static>(
        &self,
        command: impl FnOnce(oneshot::Sender<Result<T, MemoryStoreError>>) -> Command + Send + 'static,
    ) -> PortFuture<Result<T, MemoryStoreError>> {
        let sender = self.sender.clone();
        Box::pin(async move {
            let sender = sender.ok_or_else(sqlite_unavailable)?;
            let (reply, receive) = oneshot::channel();
            sender
                .send(command(reply))
                .await
                .map_err(|_| sqlite_unavailable())?;
            receive.await.map_err(|_| sqlite_unavailable())?
        })
    }

    fn from_connection(
        connection: Connection,
        memory: bool,
        clock: MemoryClock,
        limits: MemoryStoreLimits,
        store_id: &str,
    ) -> Result<Self, MemoryStoreError> {
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
        ensure_v2_auxiliary_tables(&connection)?;
        let store_id = ensure_store_identity(&connection, store_id)?;
        let (sender, receiver) = mpsc::channel(WORK_QUEUE_CAPACITY);
        let worker = std::thread::Builder::new()
            .name(String::from("finstack-memory-sqlite"))
            .spawn(move || run_worker(connection, receiver, &clock, limits))
            .map_err(|_| sqlite_unavailable())?;
        Ok(Self {
            sender: Some(sender),
            worker: Some(worker),
            store_id,
            limits,
        })
    }
}

fn run_worker(
    mut connection: Connection,
    mut receiver: mpsc::Receiver<Command>,
    clock: &MemoryClock,
    limits: MemoryStoreLimits,
) {
    while let Some(command) = receiver.blocking_recv() {
        let now = clock();
        match command {
            Command::Put { key, record, reply } => {
                let _ = reply.send(sqlite_put(&mut connection, &key, &record, now, limits));
            }
            Command::Get { scope, id, reply } => {
                let _ = reply.send(sqlite_get(&connection, &scope, &id, now));
            }
            Command::Search {
                scope,
                query,
                limit,
                reply,
            } => {
                let _ = reply.send(sqlite_search(
                    &connection,
                    &scope,
                    &query,
                    limit,
                    now,
                    limits,
                ));
            }
            Command::Forget {
                key,
                scope,
                id,
                reply,
            } => {
                let _ = reply.send(sqlite_forget(
                    &mut connection,
                    &key,
                    &scope,
                    &id,
                    now,
                    limits,
                ));
            }
            Command::Correct {
                key,
                scope,
                old,
                replacement,
                reply,
            } => {
                let _ = reply.send(sqlite_correct(
                    &mut connection,
                    &key,
                    &scope,
                    &old,
                    &replacement,
                    now,
                    limits,
                ));
            }
            Command::List { scope, page, reply } => {
                let _ = reply.send(sqlite_list(&connection, &scope, page, now, limits));
            }
            Command::PendingArtifactActions { limit, reply } => {
                let _ = reply.send(pending_artifact_actions(&connection, limit, limits));
            }
            Command::AcknowledgeArtifactAction { action_id, reply } => {
                let _ = reply.send(acknowledge_artifact_action(&connection, action_id));
            }
            Command::EmbeddingCoverage {
                scope,
                embedder_id,
                reply,
            } => {
                let _ = reply.send(super::coverage::embedding_coverage(
                    &connection,
                    &scope,
                    &embedder_id,
                    now,
                ));
            }
            Command::PendingEmbeddingSources {
                embedder_id,
                limit,
                reply,
            } => {
                let _ = reply.send(pending_embedding_sources(
                    &connection,
                    &embedder_id,
                    limit,
                    now,
                    limits,
                ));
            }
            Command::StoreEmbedding { write, reply } => {
                let _ = reply.send(store_embedding(&mut connection, &write, now, limits));
            }
            Command::ForgetEmbeddingSpace { embedder_id, reply } => {
                let _ = reply.send(forget_embedding_space(&connection, &embedder_id));
            }
        }
    }
}
