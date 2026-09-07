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
        let identity = configuration_digest("document-index-path", &location.to_string_lossy())?;
        let (sender, mut receiver) = mpsc::channel::<Job>(32);
        let join = std::thread::Builder::new()
            .name("finstack-document-index".into())
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
    if version == 1 {
        let kind: String = connection
            .query_row(
                "SELECT kind FROM document_index_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| SearchError::invalid("document_index_schema"))?;
        if kind == "finstack.document-index.v1" {
            return Ok(());
        }
    }
    if version != 0 {
        return Err(SearchError::invalid("document_index_schema_version"));
    }
    let tables: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| SearchError::SearchUnavailable)?;
    if tables != 0 {
        return Err(SearchError::invalid("document_index_foreign_database"));
    }
    let tx = connection
        .transaction()
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.execute_batch(DDL)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.pragma_update(None, "user_version", 1)
        .map_err(|_| SearchError::SearchUnavailable)?;
    tx.commit().map_err(|_| SearchError::SearchUnavailable)
}

const DDL: &str = r"
CREATE TABLE document_index_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);
INSERT INTO document_index_meta VALUES(1,'finstack.document-index.v1');
CREATE TABLE documents(
 document_key TEXT PRIMARY KEY, scope_digest TEXT NOT NULL, artifact_key TEXT NOT NULL,
 chunker_digest TEXT NOT NULL, input_json TEXT NOT NULL, report_json TEXT NOT NULL,
 UNIQUE(scope_digest,artifact_key,chunker_digest)
);
CREATE INDEX documents_scope ON documents(scope_digest,chunker_digest,document_key);
CREATE TABLE chunks(
 rowid INTEGER PRIMARY KEY, document_key TEXT NOT NULL REFERENCES documents(document_key) ON DELETE CASCADE,
 ordinal INTEGER NOT NULL, start_char INTEGER NOT NULL, end_char INTEGER NOT NULL,
 heading TEXT, text TEXT NOT NULL, content_digest TEXT NOT NULL, UNIQUE(document_key,ordinal)
);
CREATE VIRTUAL TABLE chunks_fts USING fts5(text, content='chunks', content_rowid='rowid');
CREATE TRIGGER chunks_insert AFTER INSERT ON chunks BEGIN
 INSERT INTO chunks_fts(rowid,text) VALUES(new.rowid,new.text); END;
CREATE TRIGGER chunks_delete AFTER DELETE ON chunks BEGIN
 INSERT INTO chunks_fts(chunks_fts,rowid,text) VALUES('delete',old.rowid,old.text); END;
CREATE TABLE embedding_spaces(space TEXT PRIMARY KEY,dimensions INTEGER NOT NULL);
CREATE TABLE chunk_vectors(
 chunk_id INTEGER NOT NULL REFERENCES chunks(rowid) ON DELETE CASCADE,
 space TEXT NOT NULL REFERENCES embedding_spaces(space), dimensions INTEGER NOT NULL,
 source_digest TEXT NOT NULL, vector BLOB NOT NULL, PRIMARY KEY(chunk_id,space)
);
CREATE INDEX chunk_vectors_space ON chunk_vectors(space,chunk_id);
";
