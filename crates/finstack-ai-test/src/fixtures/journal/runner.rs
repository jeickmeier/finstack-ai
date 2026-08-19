//! Historical journal v1 compatibility runner (PR-039 / A08).

use std::fs;
use std::path::{Path, PathBuf};

use finstack_ai_kernel::{APPEND_BATCH_MAX_RECORDS, Digest, RecordBody, RecordEnvelope};
use finstack_ai_protocol::{
    APPEND_BATCH_MAX_BYTES, ProtocolError, commit_record, decode, decode_value, encode,
    from_diagnostic_json, payload_digest, to_diagnostic_json, verify_chain, verify_envelope,
};
use serde::Deserialize;
use serde_json::Value;

use crate::paths::compatibility_fixture;

/// Failure loading or executing a journal v1 fixture.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum JournalFixtureError {
    /// Fixture file could not be read or written.
    #[error("journal fixture io {path}: {message}")]
    Io {
        /// Path that failed.
        path: String,
        /// OS / IO message.
        message: String,
    },
    /// Fixture JSON or expectation failed.
    #[error("{0}")]
    Failed(String),
}

/// One journal v1 sidecar.
#[derive(Debug, Clone, Deserialize)]
pub struct JournalFixture {
    /// Envelope version.
    pub format_version: u32,
    /// Subject under test.
    pub subject: String,
    /// Case slug.
    pub case: String,
    /// Expected canonical-CBOR hex when present.
    #[serde(default)]
    pub cbor_hex: Option<String>,
    /// Expected payload digest hex.
    #[serde(default)]
    pub payload_digest: Option<String>,
    /// Expected envelope checksum hex.
    #[serde(default)]
    pub checksum: Option<String>,
    /// Diagnostic JSON payload for encode/decode cases.
    #[serde(default)]
    pub diagnostic_json: Option<Value>,
    /// Deterministic materialization recipe for large/boundary payloads.
    #[serde(default)]
    pub recipe: Option<JournalRecipe>,
    /// Tamper envelopes in diagnostic JSON.
    #[serde(default)]
    pub envelopes: Option<Vec<Value>>,
    /// Claimed session head used by truncated/wrong-head cases.
    #[serde(default)]
    pub claimed_head_checksum: Option<String>,
    /// Expected outcome.
    pub expect: JournalExpect,
}

/// Deterministic constructors measured by the runner.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum JournalRecipe {
    /// Unsigned integer encoded as a decimal string.
    U64 {
        /// Decimal text of the integer.
        value: String,
    },
    /// IEEE-754 negative zero.
    NegZero,
    /// Finite float 1.0 (shortest half-precision form).
    FloatOne,
    /// Raw CBOR hex presented to the decoder.
    CborHex {
        /// Lowercase hex bytes.
        bytes: String,
    },
    /// Nested definite arrays of the requested depth plus a leaf unsigned.
    NestedArray {
        /// Number of array containers entered.
        depth: usize,
    },
    /// Definite array of `count` unsigned zeros.
    ArrayItems {
        /// Item count.
        count: usize,
    },
    /// Definite map with `count` text keys.
    MapEntries {
        /// Entry count.
        count: usize,
    },
    /// Text string of `bytes` ASCII `x` characters.
    TextString {
        /// UTF-8 length.
        bytes: usize,
    },
    /// Byte-string whose total encoded length equals `total_bytes`.
    EnvelopeBytes {
        /// Total encoded length including the CBOR header.
        total_bytes: usize,
    },
    /// Repeat one envelope `count` times and sum canonical lengths.
    BatchRecords {
        /// Record count.
        count: usize,
    },
}

/// Expected journal-fixture outcome.
#[derive(Debug, Clone, Deserialize)]
pub struct JournalExpect {
    /// Whether the case must succeed.
    pub ok: bool,
    /// Stable protocol or integrity code on failure.
    #[serde(default)]
    pub code: Option<String>,
}

/// Discover `fixtures/compatibility/journal/v1/**/*.json`.
///
/// # Errors
///
/// Returns an IO failure when the corpus root cannot be walked.
pub fn discover_journal_v1_fixtures() -> Result<Vec<PathBuf>, JournalFixtureError> {
    let root = compatibility_fixture("journal/v1");
    let mut paths = Vec::new();
    collect_json(&root, &mut paths)?;
    paths.sort();
    Ok(paths)
}

/// Load one journal sidecar.
///
/// # Errors
///
/// Returns IO or JSON failures.
pub fn load_journal_fixture(path: impl AsRef<Path>) -> Result<JournalFixture, JournalFixtureError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|error| io_err(path, error))?;
    serde_json::from_str(&text).map_err(|error| JournalFixtureError::Failed(error.to_string()))
}

/// Execute every journal v1 sidecar and return the executed count.
///
/// # Errors
///
/// Returns the first fixture failure.
pub fn run_journal_v1_corpus() -> Result<usize, JournalFixtureError> {
    let paths = discover_journal_v1_fixtures()?;
    for path in &paths {
        run_journal_fixture(path)?;
    }
    Ok(paths.len())
}

/// Execute one journal sidecar, including its sibling `.cbor` when present.
///
/// # Errors
///
/// Returns expectation or codec failures.
pub fn run_journal_fixture(path: impl AsRef<Path>) -> Result<(), JournalFixtureError> {
    let path = path.as_ref();
    let fixture = load_journal_fixture(path)?;
    if fixture.format_version != 1 {
        return Err(JournalFixtureError::Failed(format!(
            "{}: unsupported format_version",
            path.display()
        )));
    }
    match fixture.subject.as_str() {
        "cbor-profile" | "limits" => run_profile_or_limit(path, &fixture),
        "record-payload" => run_record_payload(path, &fixture),
        "envelope" => run_envelope(path, &fixture),
        "tamper" => run_tamper(path, &fixture),
        other => Err(JournalFixtureError::Failed(format!(
            "{}: unknown subject {other}",
            path.display()
        ))),
    }
}

fn run_profile_or_limit(path: &Path, fixture: &JournalFixture) -> Result<(), JournalFixtureError> {
    let outcome = execute_recipe_or_value(fixture);
    match (fixture.expect.ok, outcome) {
        (true, Ok(bytes)) => {
            assert_hex(path, fixture.cbor_hex.as_deref(), &bytes)?;
            assert_sibling_cbor(path, &bytes)?;
            Ok(())
        }
        (false, Err(code)) => assert_code(path, fixture.expect.code.as_deref(), &code),
        (true, Err(code)) => Err(JournalFixtureError::Failed(format!(
            "{}: expected success, got {code}",
            path.display()
        ))),
        (false, Ok(_)) => Err(JournalFixtureError::Failed(format!(
            "{}: expected failure {}",
            path.display(),
            fixture.expect.code.as_deref().unwrap_or("unknown")
        ))),
    }
}

fn run_record_payload(path: &Path, fixture: &JournalFixture) -> Result<(), JournalFixtureError> {
    let body = body_from_fixture(fixture)?;
    let bytes = encode(&body).map_err(|error| protocol_err(&error))?;
    let digest = payload_digest(&body).map_err(|error| protocol_err(&error))?;
    assert_hex(path, fixture.cbor_hex.as_deref(), &bytes)?;
    assert_digest(
        path,
        "payload_digest",
        fixture.payload_digest.as_deref(),
        digest,
    )?;
    assert_sibling_cbor(path, &bytes)?;
    let json = to_diagnostic_json(&body).map_err(|error| protocol_err(&error))?;
    let round: RecordBody = from_diagnostic_json(&json).map_err(|error| protocol_err(&error))?;
    if round != body {
        return Err(JournalFixtureError::Failed(format!(
            "{}: diagnostic JSON lost semantics",
            path.display()
        )));
    }
    Ok(())
}

fn run_envelope(path: &Path, fixture: &JournalFixture) -> Result<(), JournalFixtureError> {
    let envelope = envelope_from_fixture(fixture)?;
    verify_envelope(&envelope).map_err(|error| protocol_err(&error))?;
    let bytes = encode(&envelope).map_err(|error| protocol_err(&error))?;
    assert_hex(path, fixture.cbor_hex.as_deref(), &bytes)?;
    assert_digest(
        path,
        "payload_digest",
        fixture.payload_digest.as_deref(),
        envelope.payload_digest(),
    )?;
    assert_digest(
        path,
        "checksum",
        fixture.checksum.as_deref(),
        envelope.checksum(),
    )?;
    assert_sibling_cbor(path, &bytes)?;
    let json = to_diagnostic_json(&envelope).map_err(|error| protocol_err(&error))?;
    let round: RecordEnvelope =
        from_diagnostic_json(&json).map_err(|error| protocol_err(&error))?;
    if round != envelope {
        return Err(JournalFixtureError::Failed(format!(
            "{}: envelope diagnostic JSON lost semantics",
            path.display()
        )));
    }
    Ok(())
}

fn run_tamper(path: &Path, fixture: &JournalFixture) -> Result<(), JournalFixtureError> {
    let envelopes = fixture.envelopes.as_ref().ok_or_else(|| {
        JournalFixtureError::Failed(format!("{}: missing envelopes", path.display()))
    })?;
    let parsed = envelopes
        .iter()
        .map(|value| {
            serde_json::from_value::<RecordEnvelope>(value.clone())
                .map_err(|error| JournalFixtureError::Failed(error.to_string()))
        })
        .collect::<Result<Vec<_>, _>>()?;
    match verify_chain(&parsed) {
        Err(error) => {
            if fixture.expect.ok {
                Err(JournalFixtureError::Failed(format!(
                    "{}: expected success, got {}",
                    path.display(),
                    error.code()
                )))
            } else {
                assert_code(path, fixture.expect.code.as_deref(), error.code())
            }
        }
        Ok(head) => {
            if let Some(claimed) = fixture.claimed_head_checksum.as_deref() {
                let actual = head.map(|digest| digest.to_hex());
                if actual.as_deref() != Some(claimed) {
                    if fixture.expect.ok {
                        return Err(JournalFixtureError::Failed(format!(
                            "{}: claimed head unexpectedly mismatched",
                            path.display()
                        )));
                    }
                    return assert_code(
                        path,
                        fixture.expect.code.as_deref(),
                        "head_checksum_mismatch",
                    );
                }
            }
            if fixture.expect.ok {
                Ok(())
            } else {
                Err(JournalFixtureError::Failed(format!(
                    "{}: tamper fixture unexpectedly verified",
                    path.display()
                )))
            }
        }
    }
}

fn execute_recipe_or_value(fixture: &JournalFixture) -> Result<Vec<u8>, String> {
    if let Some(recipe) = &fixture.recipe {
        return execute_recipe(recipe);
    }
    let value = fixture
        .diagnostic_json
        .as_ref()
        .ok_or_else(|| "missing diagnostic_json".to_owned())?;
    encode(value).map_err(|error| error.code().to_owned())
}

fn execute_recipe(recipe: &JournalRecipe) -> Result<Vec<u8>, String> {
    match recipe {
        JournalRecipe::U64 { value } => {
            let parsed = value.parse::<u64>().map_err(|error| error.to_string())?;
            encode(&parsed).map_err(|error| error.code().to_owned())
        }
        JournalRecipe::NegZero => encode(&-0.0_f64).map_err(|error| error.code().to_owned()),
        JournalRecipe::FloatOne => encode(&1.0_f64).map_err(|error| error.code().to_owned()),
        JournalRecipe::CborHex { bytes } => {
            let raw = parse_hex(bytes)?;
            decode_value(&raw).map_err(|error| error.code().to_owned())?;
            Ok(raw)
        }
        JournalRecipe::NestedArray { depth } => {
            let mut bytes = vec![0x81; *depth];
            bytes.push(0x00);
            decode_value(&bytes).map_err(|error| error.code().to_owned())?;
            Ok(bytes)
        }
        JournalRecipe::ArrayItems { count } => {
            encode(&vec![0_u8; *count]).map_err(|error| error.code().to_owned())
        }
        JournalRecipe::MapEntries { count } => {
            let map: std::collections::BTreeMap<_, _> = (0..*count)
                .map(|index| (format!("k{index:03}"), 1_u64))
                .collect();
            encode(&map).map_err(|error| error.code().to_owned())
        }
        JournalRecipe::TextString { bytes } => {
            encode(&"x".repeat(*bytes)).map_err(|error| error.code().to_owned())
        }
        JournalRecipe::EnvelopeBytes { total_bytes } => {
            let encoded = byte_string_of_total_len(*total_bytes);
            decode::<Vec<u8>>(&encoded).map_err(|error| error.code().to_owned())?;
            Ok(encoded)
        }
        JournalRecipe::BatchRecords { count } => {
            let body = RecordBody::SessionCreated(finstack_ai_kernel::SessionCreated::new(
                finstack_ai_kernel::Metadata::empty(),
            ));
            let draft = crate::fixtures::journal::bodies::draft_for_body(body, 1)?;
            let envelope =
                commit_record(&draft, 1, None, None).map_err(|error| error.code().to_owned())?;
            let encoded = encode(&envelope).map_err(|error| error.code().to_owned())?;
            if *count > APPEND_BATCH_MAX_RECORDS {
                return Err("canonical_limit_exceeded".into());
            }
            let total = encoded
                .len()
                .checked_mul(*count)
                .ok_or_else(|| "canonical_limit_exceeded".to_owned())?;
            if total > APPEND_BATCH_MAX_BYTES {
                return Err("canonical_limit_exceeded".into());
            }
            Ok(encoded)
        }
    }
}

fn body_from_fixture(fixture: &JournalFixture) -> Result<RecordBody, JournalFixtureError> {
    let value = fixture.diagnostic_json.as_ref().ok_or_else(|| {
        JournalFixtureError::Failed("record-payload fixture missing diagnostic_json".into())
    })?;
    serde_json::from_value(value.clone())
        .map_err(|error| JournalFixtureError::Failed(error.to_string()))
}

fn envelope_from_fixture(fixture: &JournalFixture) -> Result<RecordEnvelope, JournalFixtureError> {
    let value = fixture.diagnostic_json.as_ref().ok_or_else(|| {
        JournalFixtureError::Failed("envelope fixture missing diagnostic_json".into())
    })?;
    serde_json::from_value(value.clone())
        .map_err(|error| JournalFixtureError::Failed(error.to_string()))
}

fn assert_hex(
    path: &Path,
    expected: Option<&str>,
    bytes: &[u8],
) -> Result<(), JournalFixtureError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    let actual = to_hex(bytes);
    if actual != expected {
        return Err(JournalFixtureError::Failed(format!(
            "{}: cbor_hex mismatch",
            path.display()
        )));
    }
    Ok(())
}

fn assert_digest(
    path: &Path,
    field: &str,
    expected: Option<&str>,
    actual: Digest,
) -> Result<(), JournalFixtureError> {
    let Some(expected) = expected else {
        return Ok(());
    };
    if actual.to_hex() != expected {
        return Err(JournalFixtureError::Failed(format!(
            "{}: {field} mismatch",
            path.display()
        )));
    }
    Ok(())
}

fn assert_sibling_cbor(path: &Path, bytes: &[u8]) -> Result<(), JournalFixtureError> {
    let sibling = path.with_extension("cbor");
    if !sibling.exists() {
        return Ok(());
    }
    let stored = fs::read(&sibling).map_err(|error| io_err(&sibling, error))?;
    if stored != bytes {
        return Err(JournalFixtureError::Failed(format!(
            "{}: binary fixture is not byte-identical",
            sibling.display()
        )));
    }
    Ok(())
}

fn assert_code(
    path: &Path,
    expected: Option<&str>,
    actual: &str,
) -> Result<(), JournalFixtureError> {
    match expected {
        Some(code) if code == actual => Ok(()),
        Some(code) => Err(JournalFixtureError::Failed(format!(
            "{}: expected code {code}, got {actual}",
            path.display()
        ))),
        None => Ok(()),
    }
}

fn collect_json(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), JournalFixtureError> {
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir).map_err(|error| io_err(dir, error))? {
        let entry = entry.map_err(|error| io_err(dir, error))?;
        let path = entry.path();
        if path.is_dir() {
            collect_json(&path, out)?;
        } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
            out.push(path);
        }
    }
    Ok(())
}

fn byte_string_of_total_len(total: usize) -> Vec<u8> {
    let header = 5;
    let payload = total.saturating_sub(header);
    let mut out = Vec::with_capacity(total);
    out.push(0x5a);
    out.extend_from_slice(&u32::try_from(payload).unwrap_or(u32::MAX).to_be_bytes());
    out.resize(total, 0);
    out
}

pub(crate) fn to_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        out.push(HEX[usize::from(byte >> 4)] as char);
        out.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    out
}

fn parse_hex(text: &str) -> Result<Vec<u8>, String> {
    if !text.len().is_multiple_of(2) || !text.bytes().all(|byte| byte.is_ascii_hexdigit()) {
        return Err("invalid_hex".into());
    }
    (0..text.len())
        .step_by(2)
        .map(|index| {
            u8::from_str_radix(&text[index..index + 2], 16).map_err(|error| error.to_string())
        })
        .collect()
}

pub(crate) fn protocol_err(error: &ProtocolError) -> JournalFixtureError {
    JournalFixtureError::Failed(error.to_string())
}

pub(crate) fn io_err(path: impl AsRef<Path>, error: impl std::fmt::Display) -> JournalFixtureError {
    JournalFixtureError::Io {
        path: path.as_ref().display().to_string(),
        message: error.to_string(),
    }
}

impl serde::Serialize for JournalRecipe {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        use serde::ser::SerializeMap;
        match self {
            Self::U64 { value } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "u64")?;
                map.serialize_entry("value", value)?;
                map.end()
            }
            Self::NegZero => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("kind", "neg_zero")?;
                map.end()
            }
            Self::FloatOne => {
                let mut map = serializer.serialize_map(Some(1))?;
                map.serialize_entry("kind", "float_one")?;
                map.end()
            }
            Self::CborHex { bytes } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "cbor_hex")?;
                map.serialize_entry("bytes", bytes)?;
                map.end()
            }
            Self::NestedArray { depth } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "nested_array")?;
                map.serialize_entry("depth", depth)?;
                map.end()
            }
            Self::ArrayItems { count } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "array_items")?;
                map.serialize_entry("count", count)?;
                map.end()
            }
            Self::MapEntries { count } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "map_entries")?;
                map.serialize_entry("count", count)?;
                map.end()
            }
            Self::TextString { bytes } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "text_string")?;
                map.serialize_entry("bytes", bytes)?;
                map.end()
            }
            Self::EnvelopeBytes { total_bytes } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "envelope_bytes")?;
                map.serialize_entry("total_bytes", total_bytes)?;
                map.end()
            }
            Self::BatchRecords { count } => {
                let mut map = serializer.serialize_map(Some(2))?;
                map.serialize_entry("kind", "batch_records")?;
                map.serialize_entry("count", count)?;
                map.end()
            }
        }
    }
}

/// Known-answer hex for one activated body, used by bindings.
///
/// # Errors
///
/// Returns a codec failure.
pub fn known_answer_for_body(body: &RecordBody) -> Result<(String, String), JournalFixtureError> {
    let bytes = encode(body).map_err(|error| protocol_err(&error))?;
    let digest = payload_digest(body).map_err(|error| protocol_err(&error))?;
    Ok((digest.to_hex(), to_hex(&bytes)))
}

/// Known-answer hex for one committed envelope.
///
/// # Errors
///
/// Returns a codec failure.
pub fn known_answer_for_envelope(
    envelope: &RecordEnvelope,
) -> Result<(String, String, String), JournalFixtureError> {
    let bytes = encode(envelope).map_err(|error| protocol_err(&error))?;
    Ok((
        envelope.payload_digest().to_hex(),
        envelope.checksum().to_hex(),
        to_hex(&bytes),
    ))
}
