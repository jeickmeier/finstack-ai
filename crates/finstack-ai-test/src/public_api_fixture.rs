//! Compatibility fixture runner for `public-rust-api` (PR-006/PR-007).
//!
//! Boundary recipes are materialized and measured before assertions so exact and
//! one-over ceilings are evidence, not declarations. Python/JavaScript/CBOR
//! projection fields are binding-neutral expected vectors executed by Rust.

use std::fs;
use std::path::{Path, PathBuf};

use finstack_ai_kernel::{
    AgentId, DURATION_JS_SAFE_MAX_MS, Digest, Duration, ErrorDescriptor, METADATA_MAX_MEMBERS,
    Metadata, RAW_JSON_MAX_BYTES, RawJson, RawJsonError, RunId, SessionId, TIMESTAMP_MAX_MS,
    TIMESTAMP_MIN_MS, TimeError, Timestamp,
};
use serde::Deserialize;
use serde_json::Value;

use crate::paths::compatibility_fixture;

/// Failure loading or executing a public Rust API fixture.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PublicApiFixtureError {
    /// Fixture file could not be read.
    #[error("read fixture {path}: {message}")]
    Io {
        /// Path that failed.
        path: String,
        /// OS / IO message.
        message: String,
    },
    /// Fixture JSON was invalid or failed expectations.
    #[error("{0}")]
    Failed(String),
}

/// Envelope shared by every `public-rust-api` v1 fixture.
#[derive(Debug, Clone, Deserialize)]
pub struct PublicApiFixture {
    /// Envelope version.
    pub format_version: u32,
    /// Subject under test.
    pub subject: String,
    /// Operation under test.
    pub operation: String,
    /// Literal input payload when no recipe is used.
    #[serde(default)]
    pub input: Option<Value>,
    /// Deterministic materialization recipe for large/boundary payloads.
    #[serde(default)]
    pub recipe: Option<Recipe>,
    /// Expected outcome.
    pub expect: Expect,
}

/// Deterministic payload constructors measured by the runner.
#[derive(Debug, Clone, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Recipe {
    /// JSON string `"fill"*N` sized so the full source span equals `source_bytes`.
    JsonString {
        /// Single-byte ASCII fill character.
        fill_char: char,
        /// Exact source span length including quotes.
        source_bytes: usize,
    },
    /// Nested JSON arrays of the requested depth.
    NestedArray {
        /// Number of `[` / `]` nestings.
        depth: usize,
    },
    /// Object with `count` members using keys `k0`..`k{count-1}`.
    ObjectMembers {
        /// Top-level member count.
        count: usize,
        /// Shared JSON value for every member.
        value: Value,
    },
    /// Object with a single key of `key_bytes` UTF-8 length.
    KeyBytes {
        /// Exact key length in UTF-8 bytes.
        key_bytes: usize,
    },
    /// JSON array repeating a numeric literal (used for JCS expansion).
    ScientificArray {
        /// Number of repeated leading array elements.
        count: usize,
        /// Numeric literal text (for example `1e20`).
        literal: String,
        /// Optional trailing literals joined after the repeated prefix.
        #[serde(default)]
        extras: Vec<String>,
    },
    /// Exact UTF-8 source text.
    Literal {
        /// Source text.
        text: String,
    },
}

/// Expected fixture outcome.
#[derive(Debug, Clone, Deserialize)]
pub struct Expect {
    /// Whether the operation must succeed.
    pub ok: bool,
    /// Stable error code when `ok` is false.
    #[serde(default)]
    pub error_code: Option<String>,
    /// Additional assertion fields.
    #[serde(flatten)]
    pub extras: Value,
}

/// Load a fixture JSON document.
///
/// # Errors
///
/// Returns IO or parse failures.
pub fn load_public_api_fixture(
    path: impl AsRef<Path>,
) -> Result<PublicApiFixture, PublicApiFixtureError> {
    let path = path.as_ref();
    let text = fs::read_to_string(path).map_err(|error| PublicApiFixtureError::Io {
        path: path.display().to_string(),
        message: error.to_string(),
    })?;
    serde_json::from_str(&text).map_err(|error| {
        PublicApiFixtureError::Failed(format!("parse fixture {}: {error}", path.display()))
    })
}

/// Discover all versioned `public-rust-api` fixture files.
///
/// # Errors
///
/// Returns [`PublicApiFixtureError::Io`] when a fixture directory cannot be read.
pub fn discover_public_api_fixtures() -> Result<Vec<PathBuf>, PublicApiFixtureError> {
    let root = compatibility_fixture("public-rust-api/v1");
    let mut paths = Vec::new();
    if !root.is_dir() {
        return Ok(paths);
    }
    let mut stack = vec![root];
    while let Some(dir) = stack.pop() {
        let entries = fs::read_dir(&dir).map_err(|error| PublicApiFixtureError::Io {
            path: dir.display().to_string(),
            message: error.to_string(),
        })?;
        for entry in entries {
            let entry = entry.map_err(|error| PublicApiFixtureError::Io {
                path: dir.display().to_string(),
                message: error.to_string(),
            })?;
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("json") {
                paths.push(path);
            }
        }
    }
    paths.sort();
    Ok(paths)
}

/// Execute one fixture and return `Ok(())` when expectations hold.
///
/// # Errors
///
/// Returns [`PublicApiFixtureError`] when materialization or assertions fail.
pub fn run_public_api_fixture(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    if fixture.format_version != 1 {
        return Err(PublicApiFixtureError::Failed(format!(
            "unsupported format_version {}",
            fixture.format_version
        )));
    }
    match fixture.subject.as_str() {
        "run_id" | "session_id" | "agent_id" => run_typed_id(fixture),
        "raw_json" => run_raw_json(fixture),
        "metadata" => run_metadata(fixture),
        "timestamp" => run_timestamp(fixture),
        "duration" => run_duration(fixture),
        "error_descriptor" => run_error_descriptor(fixture),
        "digest" => run_digest(fixture),
        "blob_ref" | "content_block" | "message" => {
            crate::message_fixture::run_message_subject(fixture)
        }
        "run_accepted" | "effect_requested" | "record_draft" | "append_request" | "run_event" => {
            crate::pr008_fixture::run_pr008_subject(fixture)
        }
        other => Err(PublicApiFixtureError::Failed(format!(
            "unknown subject {other}"
        ))),
    }
}

/// Load and execute every discovered fixture.
///
/// # Errors
///
/// Returns the first fixture failure.
pub fn run_all_public_api_fixtures() -> Result<usize, PublicApiFixtureError> {
    let paths = discover_public_api_fixtures()?;
    if paths.is_empty() {
        return Err(PublicApiFixtureError::Failed(
            "no public-rust-api fixtures discovered".into(),
        ));
    }
    for path in &paths {
        let fixture = load_public_api_fixture(path)?;
        run_public_api_fixture(&fixture).map_err(|error| {
            PublicApiFixtureError::Failed(format!("{}: {error}", path.display()))
        })?;
    }
    Ok(paths.len())
}

fn fail(message: impl Into<String>) -> PublicApiFixtureError {
    PublicApiFixtureError::Failed(message.into())
}

fn materialize(recipe: &Recipe) -> Result<String, PublicApiFixtureError> {
    match recipe {
        Recipe::JsonString {
            fill_char,
            source_bytes,
        } => {
            if !fill_char.is_ascii() || *fill_char == '"' || *fill_char == '\\' {
                return Err(fail(
                    "json_string fill_char must be a non-escape ASCII byte",
                ));
            }
            if *source_bytes < 2 {
                return Err(fail("json_string source_bytes must be at least 2"));
            }
            let body = fill_char.to_string().repeat(source_bytes - 2);
            let text = format!("\"{body}\"");
            if text.len() != *source_bytes {
                return Err(fail(format!(
                    "materialized json_string len {} != requested {source_bytes}",
                    text.len()
                )));
            }
            Ok(text)
        }
        Recipe::NestedArray { depth } => Ok("[".repeat(*depth) + &"]".repeat(*depth)),
        Recipe::ObjectMembers { count, value } => {
            let value_text =
                serde_json::to_string(value).map_err(|error| fail(error.to_string()))?;
            let members = (0..*count)
                .map(|idx| format!("\"k{idx}\":{value_text}"))
                .collect::<Vec<_>>()
                .join(",");
            Ok(format!("{{{members}}}"))
        }
        Recipe::KeyBytes { key_bytes } => {
            let key = "k".repeat(*key_bytes);
            if key.len() != *key_bytes {
                return Err(fail("key materialization mismatch"));
            }
            Ok(format!("{{\"{key}\":1}}"))
        }
        Recipe::ScientificArray {
            count,
            literal,
            extras,
        } => {
            let mut parts = std::iter::repeat_n(literal.as_str(), *count)
                .map(str::to_owned)
                .collect::<Vec<_>>();
            parts.extend(extras.iter().cloned());
            Ok(format!("[{}]", parts.join(",")))
        }
        Recipe::Literal { text } => Ok(text.clone()),
    }
}

fn source_bytes(fixture: &PublicApiFixture) -> Result<(String, usize), PublicApiFixtureError> {
    if let Some(recipe) = &fixture.recipe {
        let text = materialize(recipe)?;
        let len = text.len();
        if let Some(expected) = fixture.expect.extras.get("source_bytes") {
            let expected = json_usize(expected, "expect.source_bytes")?;
            if len != expected {
                return Err(fail(format!(
                    "materialized source_bytes {len} != expect.source_bytes {expected}"
                )));
            }
        }
        return Ok((text, len));
    }
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("fixture requires input or recipe"))?;
    if let Some(text) = input.as_str() {
        return Ok((text.to_owned(), text.len()));
    }
    if let Some(text) = input.get("text").and_then(Value::as_str) {
        return Ok((text.to_owned(), text.len()));
    }
    if let Some(text) = input.get("canonical").and_then(Value::as_str) {
        return Ok((text.to_owned(), text.len()));
    }
    let text = serde_json::to_string(input).map_err(|error| fail(error.to_string()))?;
    let len = text.len();
    Ok((text, len))
}

fn raw_json_error_code(error: &RawJsonError) -> &'static str {
    match error {
        RawJsonError::SourceTooLarge { .. } => "source_too_large",
        RawJsonError::CanonicalTooLarge { .. } => "canonical_too_large",
        RawJsonError::TooDeep { .. } => "too_deep",
        RawJsonError::DuplicateKey { .. } => "duplicate_key",
        RawJsonError::TrailingData => "trailing_data",
        RawJsonError::ExpectedObject => "expected_object",
        RawJsonError::TooManyMembers { .. } => "too_many_members",
        RawJsonError::KeyTooLong { .. } => "key_too_long",
        RawJsonError::Parse(_) => "parse",
        RawJsonError::Canonicalize(_) => "canonicalize",
    }
}

fn time_error_code(error: &TimeError) -> &'static str {
    match error {
        TimeError::OutOfRange { .. } => "out_of_range",
        TimeError::Overflow => "overflow",
        TimeError::InvalidRfc3339 { .. } => "invalid_rfc3339",
        TimeError::NotJsSafe { .. } => "not_js_safe",
        TimeError::InvalidJsNumber { .. } => "invalid_js_number",
    }
}

fn assert_error_code(expect: &Expect, actual: &str) -> Result<(), PublicApiFixtureError> {
    if expect.ok {
        return Err(fail(format!("expected success, got error {actual}")));
    }
    let Some(expected) = expect.error_code.as_deref() else {
        return Err(fail("failed case requires expect.error_code"));
    };
    if expected != actual {
        return Err(fail(format!(
            "error_code mismatch: expected {expected}, got {actual}"
        )));
    }
    Ok(())
}

fn run_typed_id(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("typed-id fixture requires input"))?;
    let canonical = input
        .get("canonical")
        .and_then(Value::as_str)
        .ok_or_else(|| fail("typed-id input.canonical required"))?;
    match fixture.subject.as_str() {
        "run_id" => match RunId::parse(canonical) {
            Ok(id) => {
                if !fixture.expect.ok {
                    return Err(fail("expected run_id parse failure"));
                }
                let round = id.to_canonical_string();
                let expected = fixture
                    .expect
                    .extras
                    .get("canonical")
                    .and_then(Value::as_str)
                    .unwrap_or(canonical);
                if round != expected {
                    return Err(fail(format!("canonical mismatch: {round} != {expected}")));
                }
                let json = serde_json::to_string(&id).map_err(|error| fail(error.to_string()))?;
                let de: RunId =
                    serde_json::from_str(&json).map_err(|error| fail(error.to_string()))?;
                if de != id {
                    return Err(fail("run_id serde round-trip mismatch"));
                }
                if let Some(Value::Array(reject)) = fixture.expect.extras.get("reject_as") {
                    for item in reject {
                        let name = item
                            .as_str()
                            .ok_or_else(|| fail("reject_as entries must be strings"))?;
                        // Cross-type confusion is a compile-time property; fixtures
                        // still assert that distinct parse targets remain distinct
                        // values when fed the same canonical text.
                        match name {
                            "session_id" => {
                                let session = SessionId::parse(canonical)
                                    .map_err(|error| fail(error.to_string()))?;
                                if session.to_canonical_string() != round {
                                    return Err(fail("session_id canonical diverged"));
                                }
                            }
                            other => {
                                return Err(fail(format!("unsupported reject_as {other}")));
                            }
                        }
                    }
                }
                Ok(())
            }
            Err(_) => assert_error_code(&fixture.expect, "invalid_uuid"),
        },
        "session_id" => match SessionId::parse(canonical) {
            Ok(id) => {
                if !fixture.expect.ok {
                    return Err(fail("expected session_id parse failure"));
                }
                if id.to_canonical_string() != canonical {
                    return Err(fail("session_id canonical mismatch"));
                }
                Ok(())
            }
            Err(_) => assert_error_code(&fixture.expect, "invalid_uuid"),
        },
        "agent_id" => match AgentId::parse(canonical) {
            Ok(id) => {
                if !fixture.expect.ok {
                    return Err(fail("expected agent_id parse failure"));
                }
                if id.as_str() != canonical {
                    return Err(fail("agent_id canonical mismatch"));
                }
                Ok(())
            }
            Err(_) => assert_error_code(&fixture.expect, "invalid_key"),
        },
        other => Err(fail(format!("unsupported typed-id subject {other}"))),
    }
}

fn run_raw_json(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let (text, source_len) = source_bytes(fixture)?;
    match RawJson::parse(&text) {
        Ok(raw) => {
            if !fixture.expect.ok {
                return Err(fail("expected RawJson parse failure"));
            }
            if let Some(expected) = fixture.expect.extras.get("canonical") {
                let expected = expected
                    .as_str()
                    .ok_or_else(|| fail("expect.canonical must be a string"))?;
                if raw.as_str() != expected {
                    return Err(fail(format!(
                        "canonical mismatch: {} != {expected}",
                        raw.as_str()
                    )));
                }
            }
            if let Some(expected) = fixture.expect.extras.get("canonical_bytes") {
                let expected = json_usize(expected, "expect.canonical_bytes")?;
                if raw.as_bytes().len() != expected {
                    return Err(fail(format!(
                        "canonical_bytes {} != {expected}",
                        raw.as_bytes().len()
                    )));
                }
            }
            if fixture
                .expect
                .extras
                .get("clone_shares_bytes")
                .and_then(Value::as_bool)
                == Some(true)
            {
                let clone = raw.clone();
                if !core::ptr::eq(raw.as_bytes().as_ptr(), clone.as_bytes().as_ptr()) {
                    return Err(fail("clone did not share Bytes allocation"));
                }
            }
            if source_len > RAW_JSON_MAX_BYTES {
                return Err(fail("accepted source larger than RawJson ceiling"));
            }
            Ok(())
        }
        Err(error) => {
            assert_error_code(&fixture.expect, raw_json_error_code(&error))?;
            if let Some(expected) = fixture.expect.extras.get("canonical_bytes") {
                let expected = json_usize(expected, "expect.canonical_bytes")?;
                match error {
                    RawJsonError::CanonicalTooLarge { len, .. } if len == expected => Ok(()),
                    RawJsonError::CanonicalTooLarge { len, .. } => Err(fail(format!(
                        "canonical_bytes {len} != expect.canonical_bytes {expected}"
                    ))),
                    other => Err(fail(format!(
                        "expect.canonical_bytes requires CanonicalTooLarge, got {other}"
                    ))),
                }
            } else {
                Ok(())
            }
        }
    }
}

fn run_metadata(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let (text, _) = source_bytes(fixture)?;
    match Metadata::parse(&text) {
        Ok(meta) => {
            if !fixture.expect.ok {
                return Err(fail("expected Metadata parse failure"));
            }
            if let Some(expected) = fixture.expect.extras.get("canonical") {
                let expected = expected
                    .as_str()
                    .ok_or_else(|| fail("expect.canonical must be a string"))?;
                if meta.as_str() != expected {
                    return Err(fail(format!(
                        "metadata canonical mismatch: {} != {expected}",
                        meta.as_str()
                    )));
                }
            }
            if let Some(expected) = fixture.expect.extras.get("member_count") {
                let expected = json_usize(expected, "expect.member_count")?;
                let value: Value =
                    serde_json::from_str(meta.as_str()).map_err(|error| fail(error.to_string()))?;
                let count = value
                    .as_object()
                    .ok_or_else(|| fail("metadata canonical was not an object"))?
                    .len();
                if count != expected {
                    return Err(fail(format!("member_count {count} != {expected}")));
                }
                if count > METADATA_MAX_MEMBERS {
                    return Err(fail("accepted metadata above member ceiling"));
                }
            }
            Ok(())
        }
        Err(error) => assert_error_code(&fixture.expect, raw_json_error_code(&error)),
    }
}

fn run_timestamp(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("timestamp fixture requires input"))?;
    let result = if let Some(ms) = input.get("unix_ms").and_then(Value::as_i64) {
        Timestamp::from_unix_ms(ms)
    } else if let Some(text) = input.get("rfc3339").and_then(Value::as_str) {
        Timestamp::parse_rfc3339(text)
    } else {
        return Err(fail("timestamp input needs unix_ms or rfc3339"));
    };
    match result {
        Ok(ts) => {
            if !fixture.expect.ok {
                return Err(fail("expected timestamp failure"));
            }
            assert_i64_field(&fixture.expect.extras, "unix_ms", ts.as_unix_ms())?;
            if let Some(expected) = fixture.expect.extras.get("rfc3339") {
                let expected = expected
                    .as_str()
                    .ok_or_else(|| fail("expect.rfc3339 must be a string"))?;
                if ts.to_rfc3339() != expected {
                    return Err(fail(format!(
                        "rfc3339 mismatch: {} != {expected}",
                        ts.to_rfc3339()
                    )));
                }
            }
            if let Some(expected) = fixture.expect.extras.get("cbor_integer") {
                let expected = expected
                    .as_i64()
                    .ok_or_else(|| fail("expect.cbor_integer must be an integer"))?;
                if ts.as_unix_ms() != expected {
                    return Err(fail("cbor_integer projection mismatch"));
                }
            }
            if let Some(python) = fixture.expect.extras.get("python") {
                let int_ms = python
                    .get("int_ms")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| fail("python.int_ms required"))?;
                if int_ms != ts.as_unix_ms() {
                    return Err(fail("python.int_ms mismatch"));
                }
                let aware = python
                    .get("aware_utc_datetime")
                    .and_then(Value::as_str)
                    .ok_or_else(|| fail("python.aware_utc_datetime required"))?;
                if aware != ts.to_rfc3339() {
                    return Err(fail("python.aware_utc_datetime mismatch"));
                }
            }
            if let Some(javascript) = fixture.expect.extras.get("javascript") {
                let number_ms = javascript
                    .get("number_ms")
                    .and_then(Value::as_i64)
                    .ok_or_else(|| fail("javascript.number_ms required"))?;
                if number_ms != ts.as_unix_ms() {
                    return Err(fail("javascript.number_ms mismatch"));
                }
            }
            if ts.as_unix_ms() < TIMESTAMP_MIN_MS || ts.as_unix_ms() > TIMESTAMP_MAX_MS {
                return Err(fail("timestamp escaped canonical range"));
            }
            Ok(())
        }
        Err(error) => assert_error_code(&fixture.expect, time_error_code(&error)),
    }
}

fn run_duration(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("duration fixture requires input"))?;
    if fixture.operation == "checked_add" {
        let left = read_u64_field(input, "left_ms")?;
        let right = read_u64_field(input, "right_ms")?;
        return match Duration::from_millis(left).checked_add(Duration::from_millis(right)) {
            Ok(sum) => {
                if !fixture.expect.ok {
                    return Err(fail("expected duration overflow"));
                }
                assert_u64_field(&fixture.expect.extras, "millis", sum.as_millis())
            }
            Err(error) => assert_error_code(&fixture.expect, time_error_code(&error)),
        };
    }
    let millis = read_u64_field(input, "millis")?;
    let duration = Duration::from_millis(millis);
    match duration.to_js_number() {
        Ok(number) => {
            if !fixture.expect.ok {
                return Err(fail("expected duration js conversion failure"));
            }
            assert_u64_field(&fixture.expect.extras, "millis", duration.as_millis())?;
            if let Some(expected) = fixture.expect.extras.get("javascript_number") {
                let expected = expected
                    .as_f64()
                    .ok_or_else(|| fail("javascript_number must be a number"))?;
                if (number - expected).abs() > f64::EPSILON {
                    return Err(fail("javascript_number mismatch"));
                }
            }
            if duration.as_millis() > DURATION_JS_SAFE_MAX_MS {
                return Err(fail("js-safe conversion accepted oversize duration"));
            }
            Ok(())
        }
        Err(error) => assert_error_code(&fixture.expect, time_error_code(&error)),
    }
}

fn run_error_descriptor(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let input = fixture
        .input
        .as_ref()
        .ok_or_else(|| fail("error-descriptor fixture requires input"))?;
    let descriptor: ErrorDescriptor =
        serde_json::from_value(input.clone()).map_err(|error| fail(error.to_string()))?;
    if !fixture.expect.ok {
        return Err(fail("error-descriptor parse unexpectedly succeeded"));
    }
    let json = serde_json::to_value(&descriptor).map_err(|error| fail(error.to_string()))?;
    let round: ErrorDescriptor =
        serde_json::from_value(json).map_err(|error| fail(error.to_string()))?;
    if round != descriptor {
        return Err(fail("error descriptor round-trip mismatch"));
    }
    if let Some(expected) = fixture.expect.extras.get("code") {
        let expected = expected
            .as_str()
            .ok_or_else(|| fail("expect.code must be a string"))?;
        if round.code.as_str() != expected {
            return Err(fail("error code mismatch"));
        }
    }
    if let Some(expected) = fixture.expect.extras.get("category") {
        let expected = expected
            .as_str()
            .ok_or_else(|| fail("expect.category must be a string"))?;
        if round.category.as_str() != expected {
            return Err(fail("error category mismatch"));
        }
    }
    if let Some(expected) = fixture.expect.extras.get("retryable") {
        let expected = expected
            .as_bool()
            .ok_or_else(|| fail("expect.retryable must be a bool"))?;
        if round.retryable != expected {
            return Err(fail("retryable mismatch"));
        }
    }
    if let Some(expected) = fixture.expect.extras.get("safe_details_jcs") {
        let expected = expected
            .as_str()
            .ok_or_else(|| fail("safe_details_jcs must be a string"))?;
        if round.safe_details.as_str() != expected {
            return Err(fail(format!(
                "safe_details_jcs mismatch: {} != {expected}",
                round.safe_details.as_str()
            )));
        }
    }
    // Source chains must never appear on the durable form.
    if let Value::Object(map) =
        serde_json::to_value(&round).map_err(|error| fail(error.to_string()))?
        && map.contains_key("source")
    {
        return Err(fail("ErrorDescriptor serialized a source chain"));
    }
    Ok(())
}

fn run_digest(fixture: &PublicApiFixture) -> Result<(), PublicApiFixtureError> {
    let (text, _) = source_bytes(fixture)?;
    let raw = RawJson::parse(&text).map_err(|error| fail(error.to_string()))?;
    if !fixture.expect.ok {
        return Err(fail("expected digest fixture failure"));
    }
    let digest = raw.digest();
    if let Some(expected) = fixture.expect.extras.get("digest_hex") {
        let expected = expected
            .as_str()
            .ok_or_else(|| fail("digest_hex must be a string"))?;
        if digest.to_hex() != expected {
            return Err(fail(format!(
                "digest mismatch: {} != {expected}",
                digest.to_hex()
            )));
        }
    }
    if let Some(expected) = fixture.expect.extras.get("canonical") {
        let expected = expected
            .as_str()
            .ok_or_else(|| fail("canonical must be a string"))?;
        if raw.as_str() != expected {
            return Err(fail("digest fixture canonical mismatch"));
        }
    }
    if Digest::raw_json(raw.as_bytes()) != digest {
        return Err(fail("raw_json helper disagreed with RawJson::digest"));
    }
    Ok(())
}

fn json_usize(value: &Value, label: &str) -> Result<usize, PublicApiFixtureError> {
    let number = value
        .as_u64()
        .ok_or_else(|| fail(format!("{label} must be an integer")))?;
    usize::try_from(number).map_err(|_| fail(format!("{label} exceeds usize")))
}

fn read_u64_field(input: &Value, key: &str) -> Result<u64, PublicApiFixtureError> {
    let value = input
        .get(key)
        .ok_or_else(|| fail(format!("{key} required")))?;
    if let Some(number) = value.as_u64() {
        return Ok(number);
    }
    if let Some(text) = value.as_str() {
        return text
            .parse::<u64>()
            .map_err(|error| fail(format!("invalid {key}: {error}")));
    }
    Err(fail(format!("{key} must be a u64 or decimal string")))
}

fn assert_i64_field(extras: &Value, key: &str, actual: i64) -> Result<(), PublicApiFixtureError> {
    if let Some(expected) = extras.get(key) {
        let expected = expected
            .as_i64()
            .ok_or_else(|| fail(format!("expect.{key} must be an integer")))?;
        if expected != actual {
            return Err(fail(format!("{key} mismatch: {actual} != {expected}")));
        }
    }
    Ok(())
}

fn assert_u64_field(extras: &Value, key: &str, actual: u64) -> Result<(), PublicApiFixtureError> {
    if let Some(expected) = extras.get(key) {
        let expected = expected
            .as_u64()
            .ok_or_else(|| fail(format!("expect.{key} must be an integer")))?;
        if expected != actual {
            return Err(fail(format!("{key} mismatch: {actual} != {expected}")));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use finstack_ai_kernel::{METADATA_MAX_KEY_BYTES, RAW_JSON_MAX_DEPTH};

    #[test]
    fn json_string_recipe_materializes_exact_source_span() {
        let text = materialize(&Recipe::JsonString {
            fill_char: 'a',
            source_bytes: 16,
        })
        .expect("materialize");
        assert_eq!(text.len(), 16);
        assert_eq!(text, "\"aaaaaaaaaaaaaa\"");
    }

    #[test]
    fn scientific_array_recipe_expands_under_jcs() {
        let text = materialize(&Recipe::ScientificArray {
            count: 2,
            literal: "1e20".into(),
            extras: vec!["1e10".into()],
        })
        .expect("materialize");
        assert_eq!(text, "[1e20,1e20,1e10]");
        let raw = RawJson::parse(&text).expect("parse");
        assert!(raw.as_bytes().len() > text.len());
    }

    #[test]
    fn metadata_key_ceiling_constant_is_visible() {
        assert_eq!(METADATA_MAX_KEY_BYTES, 128);
        assert_eq!(RAW_JSON_MAX_DEPTH, 32);
    }
}
