//! Owned `SQLite` index schema on the shared search-core worker shell: one
//! bounded thread owns the connection and queued cancelled requests are
//! discarded before execution.

use std::path::Path;

use finstack_ai_search_core::SearchError;
use finstack_ai_search_core::sqlite::{DatabaseSpec, Migration};

pub(crate) type Database = finstack_ai_search_core::sqlite::Database;

pub(crate) fn open(path: &Path) -> Result<Database, SearchError> {
    Database::open(path, &SPEC)
}

static SPEC: DatabaseSpec = DatabaseSpec {
    digest_label: "graph-index-path",
    thread_name: "finstack-graph-index",
    meta_table: "graph_index_meta",
    kind: "finstack.graph-index",
    version: 2,
    ddl: DDL,
    migrations: &[Migration {
        from_version: 1,
        sql: ORDERED_INDEXES,
    }],
    error_prefix: "graph_index",
};

// A fresh v2 database is the v1 tables plus the ordered indexes below.
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
CREATE INDEX entities_scope_id ON entities(scope,id);
CREATE INDEX aliases_entity_alias ON aliases(entity,alias);
";

// Preserve entity-ID/alias ordering while seeking directly into each scoped page.
// These indexes only change access paths; v1 source evidence and facts are retained.
const ORDERED_INDEXES: &str = "
CREATE INDEX entities_scope_id ON entities(scope,id);
CREATE INDEX aliases_entity_alias ON aliases(entity,alias);
";
