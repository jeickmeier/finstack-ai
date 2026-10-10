//! Owned `SQLite` index schema on the shared search-core worker shell: one
//! bounded thread owns the connection and queued cancelled requests are
//! discarded before execution.

use std::path::Path;

use finstack_ai_search_core::SearchError;
use finstack_ai_search_core::sqlite::DatabaseSpec;

pub(crate) type Database = finstack_ai_search_core::sqlite::Database;

pub(crate) fn open(path: &Path) -> Result<Database, SearchError> {
    Database::open(path, &SPEC)
}

static SPEC: DatabaseSpec = DatabaseSpec {
    digest_label: "journal-index-path",
    thread_name: "finstack-journal-index",
    meta_table: "journal_index_meta",
    kind: "finstack.journal-index",
    version: 1,
    ddl: DDL,
    migrations: &[],
    error_prefix: "journal_index",
};

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
