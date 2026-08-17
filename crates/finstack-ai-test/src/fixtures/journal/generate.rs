//! Materialize the historical journal v1 corpus (development generator).

use std::fs;
use std::path::Path;

use finstack_ai_kernel::{
    APPEND_BATCH_MAX_RECORDS, CostAmount, Digest, RecordBody, RecordEnvelope,
};
use finstack_ai_protocol::{
    CANONICAL_ARRAY_MAX_ITEMS, CANONICAL_ENVELOPE_MAX_BYTES, CANONICAL_MAP_MAX_ENTRIES,
    CANONICAL_NESTING_DEPTH, CANONICAL_STRING_MAX_BYTES, commit_record, encode, payload_digest,
};
use serde_json::Value;

use crate::fixtures::journal::bodies::{all_activated_record_bodies, draft_for_body};
use crate::fixtures::journal::runner::{
    JournalFixtureError, JournalRecipe, io_err, protocol_err, to_hex,
};
use crate::paths::repo_root;

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
