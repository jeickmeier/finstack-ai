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
        let identity = configuration_digest("journal-index-path", &location.to_string_lossy())?;
        let (sender, mut receiver) = mpsc::channel::<Job>(32);
        let join = std::thread::Builder::new()
            .name("finstack-journal-index".into())
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
                "SELECT kind FROM journal_index_meta WHERE singleton=1",
                [],
                |r| r.get(0),
            )
            .map_err(|_| SearchError::invalid("journal_index_schema"))?;
        if kind == "finstack.journal-index.v1" {
            return Ok(());
        }
    }
    if version != 0 {
        return Err(SearchError::invalid("journal_index_schema_version"));
    }
    let tables: u32 = connection
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name NOT LIKE 'sqlite_%'",
            [],
            |r| r.get(0),
        )
        .map_err(|_| SearchError::SearchUnavailable)?;
    if tables != 0 {
        return Err(SearchError::invalid("journal_index_foreign_database"));
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
CREATE TABLE journal_index_meta(singleton INTEGER PRIMARY KEY CHECK(singleton=1),kind TEXT NOT NULL);
INSERT INTO journal_index_meta VALUES(1,'finstack.journal-index.v1');
CREATE TABLE checkpoints(scope TEXT NOT NULL,session TEXT NOT NULL,state TEXT NOT NULL,PRIMARY KEY(scope,session));
CREATE TABLE entries(rowid INTEGER PRIMARY KEY,scope TEXT NOT NULL,session TEXT NOT NULL,lane TEXT NOT NULL,entry TEXT NOT NULL,timestamp TEXT,hit_json TEXT NOT NULL,text TEXT NOT NULL,complete INTEGER NOT NULL,UNIQUE(scope,session,lane,entry));
CREATE INDEX entries_scope ON entries(scope,session,lane,entry);
CREATE VIRTUAL TABLE entries_fts USING fts5(text,content='entries',content_rowid='rowid');
CREATE TRIGGER entries_insert AFTER INSERT ON entries BEGIN INSERT INTO entries_fts(rowid,text) VALUES(new.rowid,new.text); END;
CREATE TRIGGER entries_delete AFTER DELETE ON entries BEGIN INSERT INTO entries_fts(entries_fts,rowid,text) VALUES('delete',old.rowid,old.text); END;
CREATE TRIGGER entries_update AFTER UPDATE ON entries BEGIN INSERT INTO entries_fts(entries_fts,rowid,text) VALUES('delete',old.rowid,old.text); INSERT INTO entries_fts(rowid,text) VALUES(new.rowid,new.text); END;
";
