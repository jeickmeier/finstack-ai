//! One bounded worker owns this index's connection. SQL does not run on an
//! async executor; queued cancelled requests are discarded before execution.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use finstack_ai_kernel::Digest;
use finstack_ai_search_core::{SearchError, configuration_digest};
use rusqlite::Connection;
use tokio::sync::{mpsc, oneshot};

type Job = Box<dyn FnOnce(&mut Connection) + Send>;

struct Worker {
    sender: Option<mpsc::Sender<Job>>,
    join: Option<std::thread::JoinHandle<()>>,
}

impl Drop for Worker {
    fn drop(&mut self) {
        self.sender.take();
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

#[derive(Clone)]
pub(crate) struct Database {
    worker: Arc<Worker>,
    pub(crate) identity: Digest,
}

impl Database {
    pub(crate) fn open(path: &Path) -> Result<Self, SearchError> {
        let mut connection = Connection::open(path).map_err(|_| SearchError::SearchUnavailable)?;
        connection
            .busy_timeout(Duration::from_secs(1))
            .map_err(|_| SearchError::SearchUnavailable)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|_| SearchError::SearchUnavailable)?;
        if path != Path::new(":memory:") {
            connection
                .pragma_update(None, "journal_mode", "WAL")
                .map_err(|_| SearchError::SearchUnavailable)?;
        }
        schema(&mut connection)?;
        let location = if path == Path::new(":memory:") {
            path.to_path_buf()
        } else {
            std::fs::canonicalize(path).map_err(|_| SearchError::SearchUnavailable)?
        };
        let identity = configuration_digest("graph-index-path", &location.to_string_lossy())?;
        let (sender, mut receiver) = mpsc::channel::<Job>(32);
        let join = std::thread::Builder::new()
            .name("finstack-graph-index".into())
            .spawn(move || {
                while let Some(job) = receiver.blocking_recv() {
                    job(&mut connection);
                }
            })
            .map_err(|_| SearchError::SearchUnavailable)?;
        Ok(Self {
            worker: Arc::new(Worker {
                sender: Some(sender),
                join: Some(join),
            }),
            identity,
        })
    }

    pub(crate) async fn call<T: Send + 'static>(
        &self,
        operation: impl FnOnce(&mut Connection) -> Result<T, SearchError> + Send + 'static,
    ) -> Result<T, SearchError> {
        let (reply, result) = oneshot::channel();
        let job: Job = Box::new(move |connection| {
            if !reply.is_closed() {
                let _ = reply.send(operation(connection));
            }
        });
        self.worker
            .sender
            .as_ref()
            .ok_or(SearchError::SearchUnavailable)?
            .try_send(job)
            .map_err(|_| SearchError::SearchUnavailable)?;
        result.await.map_err(|_| SearchError::SearchUnavailable)?
    }
}

fn schema(connection: &mut Connection) -> Result<(), SearchError> {
    let version: u32 = connection
        .pragma_query_value(None, "user_version", |row| row.get(0))
        .map_err(|_| SearchError::SearchUnavailable)?;
    if version == 1 || version == 2 {
        let kind: String = connection
            .query_row(
                "SELECT kind FROM graph_index_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| SearchError::invalid("graph_index_schema"))?;
        if version == 2 && kind == "finstack.graph-index.v2" {
            return Ok(());
        }
        if version == 1 && kind == "finstack.graph-index.v1" {
            let tx = connection
                .transaction()
                .map_err(|_| SearchError::SearchUnavailable)?;
            tx.execute_batch(ORDERED_INDEXES)
                .map_err(|_| SearchError::SearchUnavailable)?;
            tx.execute(
                "UPDATE graph_index_meta SET kind='finstack.graph-index.v2' WHERE singleton=1",
                [],
            )
            .map_err(|_| SearchError::SearchUnavailable)?;
            tx.pragma_update(None, "user_version", 2)
                .map_err(|_| SearchError::SearchUnavailable)?;
            return tx.commit().map_err(|_| SearchError::SearchUnavailable);
        }
    }
    if version != 0 {
        return Err(SearchError::invalid("graph_index_schema_version"));
    }
    let tables: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| SearchError::SearchUnavailable)?;
    if tables != 0 {
        return Err(SearchError::invalid("graph_index_foreign_database"));
    }
    let tx = connection
        .transaction()
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.execute_batch(DDL)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.execute_batch(ORDERED_INDEXES)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.pragma_update(None, "user_version", 2)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.commit().map_err(|_| SearchError::SearchUnavailable)
}

const DDL: &str = r"
CREATE TABLE graph_index_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);
INSERT INTO graph_index_meta VALUES(1,'finstack.graph-index.v2');
CREATE TABLE sources(source_key TEXT PRIMARY KEY,scope TEXT NOT NULL,source_id TEXT NOT NULL,reference TEXT NOT NULL,hit_json TEXT NOT NULL,complete INTEGER NOT NULL,extraction_digest TEXT NOT NULL);
CREATE INDEX sources_scope ON sources(scope,source_key);
CREATE TABLE entities(id TEXT PRIMARY KEY,scope TEXT NOT NULL,kind TEXT NOT NULL,label TEXT NOT NULL,UNIQUE(scope,kind,label));
CREATE INDEX entities_scope ON entities(scope,kind,label,id);
CREATE TABLE entity_sources(entity TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,source_key TEXT NOT NULL REFERENCES sources(source_key) ON DELETE CASCADE,PRIMARY KEY(entity,source_key));
CREATE TABLE aliases(entity TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,source_key TEXT NOT NULL REFERENCES sources(source_key) ON DELETE CASCADE,alias TEXT NOT NULL,PRIMARY KEY(entity,source_key,alias));
CREATE INDEX aliases_lookup ON aliases(alias,entity);
CREATE TABLE edges(id TEXT PRIMARY KEY,scope TEXT NOT NULL,src TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,kind TEXT NOT NULL,dst TEXT NOT NULL REFERENCES entities(id) ON DELETE CASCADE,UNIQUE(scope,src,kind,dst));
CREATE INDEX edges_outgoing ON edges(scope,src,id);
CREATE INDEX edges_incoming ON edges(scope,dst,id);
CREATE TABLE edge_sources(edge TEXT NOT NULL REFERENCES edges(id) ON DELETE CASCADE,source_key TEXT NOT NULL REFERENCES sources(source_key) ON DELETE CASCADE,PRIMARY KEY(edge,source_key));
";

// Preserve entity-ID/alias ordering while seeking directly into each scoped page.
// These indexes only change access paths; v1 source evidence and facts are retained.
const ORDERED_INDEXES: &str = "
CREATE INDEX entities_scope_id ON entities(scope,id);
CREATE INDEX aliases_entity_alias ON aliases(entity,alias);
";
