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
    digest_label: "document-index-path",
    thread_name: "finstack-document-index",
    meta_table: "document_index_meta",
    kind: "finstack.document-index",
    version: 1,
    ddl: DDL,
    migrations: &[],
    error_prefix: "document_index",
};

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
