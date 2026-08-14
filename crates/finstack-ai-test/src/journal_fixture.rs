//! Historical journal v1 compatibility runner (PR-039 / A08).

use std::fs;
use std::path::{Path, PathBuf};

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, CostAmount, Digest, RecordBody, RecordEnvelope,
};
use finstack_ai_protocol::{
    APPEND_BATCH_MAX_BYTES, CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES,
    CANONICAL_MAP_MAX_ENTRIES, CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES, ProtocolError,
    commit_record, decode, decode_value, encode, from_diagnostic_json, payload_digest,
    to_diagnostic_json, verify_chain, verify_envelope,
};
use serde::Deserialize;
use serde_json::Value;

use crate::journal_bodies::{all_activated_record_bodies, draft_for_body};
use crate::paths::{compatibility_fixture, repo_root};

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

/// Materialize the first historical journal v1 corpus.
///
/// # Errors
///
/// Returns construction or IO failures.
pub fn write_journal_v1_fixtures() -> Result<usize, JournalFixtureError> {
    let root = repo_root().join("fixtures/compatibility/journal/v1");
    fs::create_dir_all(root.join("cbor-profile")).map_err(|error| io_err(&root, error))?;
    fs::create_dir_all(root.join("record-payload")).map_err(|error| io_err(&root, error))?;
    fs::create_dir_all(root.join("envelope")).map_err(|error| io_err(&root, error))?;
    fs::create_dir_all(root.join("limits")).map_err(|error| io_err(&root, error))?;
    fs::create_dir_all(root.join("tamper")).map_err(|error| io_err(&root, error))?;

    let mut written = 0;
    written += write_cbor_profile(&root)?;
    written += write_limits(&root)?;
    written += write_record_payloads(&root)?;
    written += write_envelopes(&root)?;
    written += write_tamper(&root)?;
    Ok(written)
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
            let draft = crate::journal_bodies::draft_for_body(body, 1)?;
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

fn write_cbor_profile(root: &Path) -> Result<usize, JournalFixtureError> {
    let dir = root.join("cbor-profile");
    write_cbor_map_order(&dir)?;
    write_cbor_integers(&dir)?;
    write_cbor_floats(&dir)?;
    write_cbor_rejections(&dir)?;
    write_cbor_cost_amount(&dir)?;
    Ok(11)
}

fn write_cbor_map_order(dir: &Path) -> Result<(), JournalFixtureError> {
    let map = serde_json::json!({"a": 2, "b": 1});
    let map_bytes = encode(&map).map_err(|error| protocol_err(&error))?;
    write_case(
        dir,
        "valid--map-order.json",
        json_ok(
            "cbor-profile",
            "map-order",
            Some(&map_bytes),
            None,
            None,
            Some(map),
            None,
        ),
        Some(&map_bytes),
    )
}

fn write_cbor_integers(dir: &Path) -> Result<(), JournalFixtureError> {
    write_int(dir, "valid--integer-0.json", "integer-0", 0)?;
    write_int(
        dir,
        "valid--integer-2-pow-53-minus-1.json",
        "integer-2-pow-53-minus-1",
        (1_u64 << 53) - 1,
    )?;
    write_int(
        dir,
        "valid--integer-2-pow-53.json",
        "integer-2-pow-53",
        1_u64 << 53,
    )?;
    write_int(
        dir,
        "valid--integer-u64-max.json",
        "integer-u64-max",
        u64::MAX,
    )
}

fn write_cbor_floats(dir: &Path) -> Result<(), JournalFixtureError> {
    let one = encode(&1.0_f64).map_err(|error| protocol_err(&error))?;
    write_case(
        dir,
        "valid--float-one.json",
        recipe_ok(
            "cbor-profile",
            "float-one",
            Some(&one),
            JournalRecipe::FloatOne,
        ),
        Some(&one),
    )?;
    let neg = encode(&-0.0_f64).map_err(|error| protocol_err(&error))?;
    write_case(
        dir,
        "valid--neg-zero.json",
        recipe_ok(
            "cbor-profile",
            "neg-zero",
            Some(&neg),
            JournalRecipe::NegZero,
        ),
        Some(&neg),
    )
}

fn write_cbor_rejections(dir: &Path) -> Result<(), JournalFixtureError> {
    write_invalid(
        dir,
        "cbor-profile",
        "invalid--bignum-tag-2.json",
        "bignum-tag-2",
        JournalRecipe::CborHex {
            bytes: "c24101".into(),
        },
        "bignum_tag",
    )?;
    write_invalid(
        dir,
        "cbor-profile",
        "invalid--bignum-tag-3.json",
        "bignum-tag-3",
        JournalRecipe::CborHex {
            bytes: "c34101".into(),
        },
        "bignum_tag",
    )?;
    write_invalid(
        dir,
        "cbor-profile",
        "invalid--non-finite-nan.json",
        "non-finite-nan",
        JournalRecipe::CborHex {
            bytes: "f97e00".into(),
        },
        "non_finite_float",
    )
}

fn write_cbor_cost_amount(dir: &Path) -> Result<(), JournalFixtureError> {
    let amount = CostAmount::try_new("USD", u64::MAX, "price-v1")
        .map_err(|error| JournalFixtureError::Failed(error.to_string()))?;
    let amount_json = serde_json::to_value(&amount)
        .map_err(|error| JournalFixtureError::Failed(error.to_string()))?;
    let amount_bytes = encode(&amount).map_err(|error| protocol_err(&error))?;
    write_case(
        dir,
        "valid--cost-amount-micros.json",
        json_ok(
            "cbor-profile",
            "cost-amount-micros",
            Some(&amount_bytes),
            None,
            None,
            Some(amount_json),
            None,
        ),
        Some(&amount_bytes),
    )
}

fn write_limits(root: &Path) -> Result<usize, JournalFixtureError> {
    let dir = root.join("limits");
    write_limit_ok(
        &dir,
        "valid--array-items-exact.json",
        "array-items-exact",
        JournalRecipe::ArrayItems {
            count: CANONICAL_ARRAY_MAX_ITEMS,
        },
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--array-items-one-over.json",
        "array-items-one-over",
        JournalRecipe::ArrayItems {
            count: CANONICAL_ARRAY_MAX_ITEMS + 1,
        },
        "canonical_limit_exceeded",
    )?;
    write_limit_ok(
        &dir,
        "valid--map-entries-exact.json",
        "map-entries-exact",
        JournalRecipe::MapEntries {
            count: CANONICAL_MAP_MAX_ENTRIES,
        },
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--map-entries-one-over.json",
        "map-entries-one-over",
        JournalRecipe::MapEntries {
            count: CANONICAL_MAP_MAX_ENTRIES + 1,
        },
        "canonical_limit_exceeded",
    )?;
    write_limit_ok(
        &dir,
        "valid--text-string-exact.json",
        "text-string-exact",
        JournalRecipe::TextString {
            bytes: CANONICAL_STRING_MAX_BYTES,
        },
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--text-string-one-over.json",
        "text-string-one-over",
        JournalRecipe::TextString {
            bytes: CANONICAL_STRING_MAX_BYTES + 1,
        },
        "canonical_limit_exceeded",
    )?;
    write_limit_ok(
        &dir,
        "valid--nesting-exact.json",
        "nesting-exact",
        JournalRecipe::NestedArray {
            depth: CANONICAL_NESTING_DEPTH,
        },
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--nesting-one-over.json",
        "nesting-one-over",
        JournalRecipe::NestedArray {
            depth: CANONICAL_NESTING_DEPTH + 1,
        },
        "canonical_limit_exceeded",
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--envelope-bytes-one-over.json",
        "envelope-bytes-one-over",
        JournalRecipe::EnvelopeBytes {
            total_bytes: CANONICAL_ENVELOPE_MAX_BYTES + 1,
        },
        "canonical_limit_exceeded",
    )?;
    write_invalid(
        &dir,
        "limits",
        "invalid--batch-records-one-over.json",
        "batch-records-one-over",
        JournalRecipe::BatchRecords {
            count: APPEND_BATCH_MAX_RECORDS + 1,
        },
        "canonical_limit_exceeded",
    )?;
    Ok(10)
}

fn write_record_payloads(root: &Path) -> Result<usize, JournalFixtureError> {
    let dir = root.join("record-payload");
    let bodies = all_activated_record_bodies().map_err(JournalFixtureError::Failed)?;
    for body in &bodies {
        let bytes = encode(body).map_err(|error| protocol_err(&error))?;
        let digest = payload_digest(body).map_err(|error| protocol_err(&error))?;
        let diagnostic = serde_json::to_value(body)
            .map_err(|error| JournalFixtureError::Failed(error.to_string()))?;
        write_case(
            &dir,
            &format!("valid--{}.json", body.kind_name().replace('_', "-")),
            json_ok(
                "record-payload",
                body.kind_name(),
                Some(&bytes),
                Some(&digest),
                None,
                Some(diagnostic),
                None,
            ),
            Some(&bytes),
        )?;
    }
    Ok(bodies.len())
}

fn write_envelopes(root: &Path) -> Result<usize, JournalFixtureError> {
    let dir = root.join("envelope");
    let bodies = all_activated_record_bodies().map_err(JournalFixtureError::Failed)?;
    for (ordinal, body) in bodies.iter().enumerate() {
        let envelope =
            committed_envelope(body.clone(), u64::try_from(ordinal + 1).expect("ord"), None)?;
        let bytes = encode(&envelope).map_err(|error| protocol_err(&error))?;
        let diagnostic = serde_json::to_value(&envelope)
            .map_err(|error| JournalFixtureError::Failed(error.to_string()))?;
        write_case(
            &dir,
            &format!("valid--{}.json", body.kind_name().replace('_', "-")),
            json_ok(
                "envelope",
                body.kind_name(),
                Some(&bytes),
                Some(&envelope.payload_digest()),
                Some(&envelope.checksum()),
                Some(diagnostic),
                None,
            ),
            Some(&bytes),
        )?;
    }
    Ok(bodies.len())
}

fn write_tamper(root: &Path) -> Result<usize, JournalFixtureError> {
    let dir = root.join("tamper");
    let first = committed_envelope(
        RecordBody::SessionCreated(finstack_ai_kernel::SessionCreated::new(
            finstack_ai_kernel::Metadata::empty(),
        )),
        1,
        None,
    )?;
    let second = committed_envelope(
        RecordBody::LaneCreated(
            finstack_ai_kernel::LaneCreated::try_new("main")
                .map_err(|error| JournalFixtureError::Failed(error.to_string()))?,
        ),
        2,
        Some(first.checksum()),
    )?;
    write_tamper_case(
        &dir,
        "invalid--reordered.json",
        "reordered",
        vec![json_envelope(&second)?, json_envelope(&first)?],
        "checksum_chain_break",
    )?;
    write_tamper_case_with_head(
        &dir,
        "invalid--truncated.json",
        "truncated",
        vec![json_envelope(&first)?],
        Some(second.checksum().to_hex()),
        "head_checksum_mismatch",
    )?;
    let mut modified = json_envelope(&first)?;
    modified["body"]["session_created"]["metadata"] = serde_json::json!({"tampered": true});
    write_tamper_case(
        &dir,
        "invalid--payload-modified.json",
        "payload-modified",
        vec![modified],
        "payload_digest_mismatch",
    )?;
    let mut wrong_head = json_envelope(&first)?;
    wrong_head["checksum"] = serde_json::json!(Digest::raw_json(b"wrong").to_hex());
    write_tamper_case(
        &dir,
        "invalid--wrong-head.json",
        "wrong-head",
        vec![wrong_head],
        "envelope_checksum_mismatch",
    )?;
    Ok(4)
}

fn committed_envelope(
    body: RecordBody,
    ordinal: u64,
    previous: Option<Digest>,
) -> Result<RecordEnvelope, JournalFixtureError> {
    let draft = draft_for_body(body, ordinal).map_err(JournalFixtureError::Failed)?;
    commit_record(&draft, ordinal, previous, None).map_err(|error| protocol_err(&error))
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

fn write_int(dir: &Path, name: &str, case: &str, value: u64) -> Result<(), JournalFixtureError> {
    let bytes = encode(&value).map_err(|error| protocol_err(&error))?;
    write_case(
        dir,
        name,
        recipe_ok(
            "cbor-profile",
            case,
            Some(&bytes),
            JournalRecipe::U64 {
                value: value.to_string(),
            },
        ),
        Some(&bytes),
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "one-shot fixture writers own their payloads"
)]
fn write_limit_ok(
    dir: &Path,
    name: &str,
    case: &str,
    recipe: JournalRecipe,
) -> Result<(), JournalFixtureError> {
    write_case(dir, name, recipe_ok("limits", case, None, recipe), None)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "one-shot fixture writers own their payloads"
)]
fn write_invalid(
    dir: &Path,
    subject: &str,
    name: &str,
    case: &str,
    recipe: JournalRecipe,
    code: &str,
) -> Result<(), JournalFixtureError> {
    write_case(
        dir,
        name,
        serde_json::json!({
            "format_version": 1,
            "subject": subject,
            "case": case,
            "recipe": recipe,
            "expect": { "ok": false, "code": code }
        }),
        None,
    )
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "one-shot fixture writers own their payloads"
)]
fn write_tamper_case(
    dir: &Path,
    name: &str,
    case: &str,
    envelopes: Vec<Value>,
    code: &str,
) -> Result<(), JournalFixtureError> {
    write_tamper_case_with_head(dir, name, case, envelopes, None, code)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "one-shot fixture writers own their payloads"
)]
fn write_tamper_case_with_head(
    dir: &Path,
    name: &str,
    case: &str,
    envelopes: Vec<Value>,
    claimed_head_checksum: Option<String>,
    code: &str,
) -> Result<(), JournalFixtureError> {
    let mut value = serde_json::json!({
        "format_version": 1,
        "subject": "tamper",
        "case": case,
        "envelopes": envelopes,
        "expect": { "ok": false, "code": code }
    });
    if let Some(head) = claimed_head_checksum {
        value["claimed_head_checksum"] = Value::String(head);
    }
    write_case(dir, name, value, None)
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "one-shot fixture writers own their payloads"
)]
fn write_case(
    dir: &Path,
    name: &str,
    value: Value,
    cbor: Option<&[u8]>,
) -> Result<(), JournalFixtureError> {
    let path = dir.join(name);
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| JournalFixtureError::Failed(error.to_string()))?;
    fs::write(&path, format!("{text}\n")).map_err(|error| io_err(&path, error))?;
    if let Some(bytes) = cbor {
        let cbor_path = path.with_extension("cbor");
        fs::write(&cbor_path, bytes).map_err(|error| io_err(&cbor_path, error))?;
    }
    Ok(())
}

fn json_ok(
    subject: &str,
    case: &str,
    cbor: Option<&[u8]>,
    payload: Option<&Digest>,
    checksum: Option<&Digest>,
    diagnostic_json: Option<Value>,
    recipe: Option<JournalRecipe>,
) -> Value {
    let mut value = serde_json::json!({
        "format_version": 1,
        "subject": subject,
        "case": case,
        "expect": { "ok": true }
    });
    if let Some(bytes) = cbor {
        value["cbor_hex"] = Value::String(to_hex(bytes));
    }
    if let Some(digest) = payload {
        value["payload_digest"] = Value::String(digest.to_hex());
    }
    if let Some(digest) = checksum {
        value["checksum"] = Value::String(digest.to_hex());
    }
    if let Some(json) = diagnostic_json {
        value["diagnostic_json"] = json;
    }
    if let Some(recipe) = recipe {
        value["recipe"] = serde_json::to_value(recipe).expect("recipe");
    }
    value
}

fn recipe_ok(subject: &str, case: &str, cbor: Option<&[u8]>, recipe: JournalRecipe) -> Value {
    json_ok(subject, case, cbor, None, None, None, Some(recipe))
}

fn json_envelope(envelope: &RecordEnvelope) -> Result<Value, JournalFixtureError> {
    serde_json::to_value(envelope).map_err(|error| JournalFixtureError::Failed(error.to_string()))
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
    out.extend_from_slice(
        &u32::try_from(payload)
            .expect("payload fits u32")
            .to_be_bytes(),
    );
    out.resize(total, 0);
    out
}

fn to_hex(bytes: &[u8]) -> String {
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

fn protocol_err(error: &ProtocolError) -> JournalFixtureError {
    JournalFixtureError::Failed(error.to_string())
}

fn io_err(path: impl AsRef<Path>, error: impl std::fmt::Display) -> JournalFixtureError {
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
