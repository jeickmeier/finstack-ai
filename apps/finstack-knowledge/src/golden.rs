//! The shared golden-questions fixture.
//!
//! One fixture, three surfaces: the Rust tests here, the Python golden
//! pytest, and the browser example's Playwright spec all load
//! `fixtures/golden.json` and make the same two assertions per entry —
//! the answer contains `must_contain`, and the observed event kinds are a
//! superset of `event_kinds_expected`. Drift between surfaces is a CI
//! failure, not a discovery.

use serde::Deserialize;

use crate::KnowledgeError;

/// One golden question with its offline script and expectations.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GoldenEntry {
    /// Stable unique id.
    pub id: String,
    /// The question a surface asks.
    pub question: String,
    /// The response the offline scripted model returns.
    pub scripted_response: String,
    /// Substrings the final answer must contain.
    pub must_contain: Vec<String>,
    /// Event kinds every surface must observe (subset check).
    pub event_kinds_expected: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct GoldenFixture {
    entries: Vec<GoldenEntry>,
}

/// Load and validate the embedded golden fixture.
///
/// # Errors
///
/// Returns [`KnowledgeError::Config`] when the fixture fails to parse,
/// contains duplicate or empty ids, or an entry has an empty field.
pub fn golden_entries() -> Result<Vec<GoldenEntry>, KnowledgeError> {
    let fixture: GoldenFixture = serde_json::from_str(include_str!("../fixtures/golden.json"))
        .map_err(|_| KnowledgeError::Config {
            reason: "golden_fixture_unparseable",
        })?;
    let entries = fixture.entries;
    let mut ids: Vec<&str> = entries.iter().map(|entry| entry.id.as_str()).collect();
    ids.sort_unstable();
    ids.dedup();
    if ids.len() != entries.len() {
        return Err(KnowledgeError::Config {
            reason: "golden_fixture_duplicate_id",
        });
    }
    for entry in &entries {
        if entry.id.trim().is_empty()
            || entry.question.trim().is_empty()
            || entry.scripted_response.trim().is_empty()
            || entry.must_contain.is_empty()
            || entry.event_kinds_expected.is_empty()
        {
            return Err(KnowledgeError::Config {
                reason: "golden_fixture_entry_invalid",
            });
        }
    }
    Ok(entries)
}
