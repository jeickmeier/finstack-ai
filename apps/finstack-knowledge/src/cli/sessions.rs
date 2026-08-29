//! `sessions list | show | name` over the shared sqlite journal.
//!
//! These commands consume only the store's `list_sessions` convenience and
//! the `JournalStore` port (`load`, `write_metadata`) plus `Session`/`Lane`
//! inspection — no journal parsing happens here.

use std::fmt::Write as _;

use finstack_ai::runtime::ports::journal::{LoadRequest, WriteMetadataRequest};
use finstack_ai_kernel::{ContentBlock, EntryBody, Metadata, SessionId};
use rich_rust::console::Console;
use rich_rust::markup::escape;
use rich_rust::renderables::{Cell, Column, Row, Table};

use crate::{KnowledgeConfig, KnowledgeError};

/// Metadata key holding the human-readable session name.
const NAME_KEY: &str = "finstack.know.name";

/// Sessions per listing page.
const LIST_LIMIT: u32 = 256;

/// Render the session listing as a table (plain text).
///
/// # Errors
///
/// Returns [`KnowledgeError`] when the journal cannot be opened or listed.
pub async fn run_list(config: &KnowledgeConfig) -> Result<String, KnowledgeError> {
    let store = crate::open_journal_sqlite(config)?;
    let mut rows = store.list_sessions(LIST_LIMIT).await.map_err(store_error)?;
    rows.sort_by_key(|row| std::cmp::Reverse(row.head_sequence));

    let mut table = Table::new();
    table.add_column(Column::new("session"));
    table.add_column(Column::new("name"));
    table.add_column(Column::new("head"));
    for row in &rows {
        table.add_row(Row::new(vec![
            Cell::new(row.session_id.to_string()),
            Cell::new(session_name(&row.metadata).unwrap_or_default()),
            Cell::new(row.head_sequence.to_string()),
        ]));
    }
    let console = Console::builder().markup(true).build();
    Ok(console.export_renderable_text(&table))
}

/// Serialize the listing as JSON rows (for `--json`).
///
/// # Errors
///
/// Returns [`KnowledgeError`] when the journal cannot be opened or listed.
pub async fn run_list_json(config: &KnowledgeConfig) -> Result<String, KnowledgeError> {
    let store = crate::open_journal_sqlite(config)?;
    let rows = store.list_sessions(LIST_LIMIT).await.map_err(store_error)?;
    let mut lines = String::new();
    for row in rows {
        let value = serde_json::json!({
            "session_id": row.session_id.to_string(),
            "name": session_name(&row.metadata),
            "head_sequence": row.head_sequence,
        });
        lines.push_str(&value.to_string());
        lines.push('\n');
    }
    Ok(lines)
}

/// Render one session's lanes and conversation history (plain text).
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] for an unparseable id and
/// [`KnowledgeError::Compose`] when the session cannot be opened.
pub async fn run_show(config: &KnowledgeConfig, id: &str) -> Result<String, KnowledgeError> {
    let journal = crate::open_journal(config)?;
    let session_id = SessionId::parse(id).map_err(|_| KnowledgeError::Config {
        reason: "session_id_invalid",
    })?;
    let session = finstack_ai::Session::open(journal, session_id, "local")
        .await
        .map_err(compose)?;
    let mut markup = String::new();
    for lane in session.list_lanes().await.map_err(compose)? {
        let inspect = lane.inspect().await.map_err(compose)?;
        let _ = writeln!(markup, "[bold]lane {}[/bold]", escape(&inspect.name));
        for entry in &inspect.history {
            match entry.body() {
                EntryBody::Message(message) => {
                    let text: String = message
                        .content()
                        .iter()
                        .filter_map(|block| match block {
                            ContentBlock::Text(text) => Some(text.text()),
                            _ => None,
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    let _ = writeln!(
                        markup,
                        "  [cyan]{}[/cyan] {}",
                        message.role().as_str(),
                        escape(text.trim())
                    );
                }
            }
        }
    }
    let console = Console::builder().markup(true).build();
    Ok(console.export_text(&markup))
}

/// Name a session via metadata compare-and-swap, retrying once on conflict.
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] for an unparseable id or name and
/// [`KnowledgeError::Compose`] when the CAS fails twice.
pub async fn run_name(
    config: &KnowledgeConfig,
    id: &str,
    name: &str,
) -> Result<(), KnowledgeError> {
    if name.trim().is_empty() {
        return Err(KnowledgeError::Config {
            reason: "session_name_empty",
        });
    }
    let journal = crate::open_journal(config)?;
    let session_id = SessionId::parse(id).map_err(|_| KnowledgeError::Config {
        reason: "session_id_invalid",
    })?;
    for attempt in 0..2_u8 {
        let loaded = journal
            .load(LoadRequest { session_id })
            .await
            .map_err(store_error)?;
        let metadata = with_name(&loaded.metadata, name)?;
        let result = journal
            .write_metadata(WriteMetadataRequest {
                session_id,
                expected_head_checksum: loaded.head_checksum,
                metadata,
            })
            .await;
        match result {
            Ok(_) => return Ok(()),
            Err(error) if attempt == 0 => {
                // CAS conflict: reload the fresh head once and retry.
                let _ = error;
            }
            Err(error) => return Err(store_error(error)),
        }
    }
    Err(KnowledgeError::Compose {
        reason: "session_name_cas_conflict".to_owned(),
    })
}

/// Read the stored name from session metadata, if any.
fn session_name(metadata: &Metadata) -> Option<String> {
    let value: serde_json::Value = serde_json::from_slice(metadata.as_bytes()).ok()?;
    value
        .get(NAME_KEY)
        .and_then(serde_json::Value::as_str)
        .map(str::to_owned)
}

/// Merge the name key into existing metadata without dropping other keys.
fn with_name(metadata: &Metadata, name: &str) -> Result<Metadata, KnowledgeError> {
    let mut value: serde_json::Value =
        serde_json::from_slice(metadata.as_bytes()).unwrap_or_else(|_| serde_json::json!({}));
    if !value.is_object() {
        value = serde_json::json!({});
    }
    if let Some(object) = value.as_object_mut() {
        object.insert(
            NAME_KEY.to_owned(),
            serde_json::Value::String(name.to_owned()),
        );
    }
    Metadata::parse(value.to_string().as_bytes()).map_err(|_| KnowledgeError::Config {
        reason: "session_name_invalid",
    })
}

fn compose(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Compose {
        reason: error.to_string(),
    }
}

fn store_error(error: impl std::fmt::Display) -> KnowledgeError {
    KnowledgeError::Compose {
        reason: error.to_string(),
    }
}
