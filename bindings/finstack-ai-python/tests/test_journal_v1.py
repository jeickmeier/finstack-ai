"""PR-039 known-answer digest checks through the one Rust engine."""

from __future__ import annotations

import json
from pathlib import Path

import finstack_ai

_REPO_ROOT = Path(__file__).resolve().parents[3]
_JOURNAL_V1 = _REPO_ROOT / "fixtures/compatibility/journal/v1"


def test_record_body_known_answers_match_rust_fixtures() -> None:
    payloads = sorted((_JOURNAL_V1 / "record-payload").glob("valid--*.json"))
    assert len(payloads) == 39
    for path in payloads:
        fixture = json.loads(path.read_text())
        answer = finstack_ai.journal_known_answer(
            "record_body", fixture["diagnostic_json"]
        )
        assert answer["payload_digest"] == fixture["payload_digest"]
        assert answer["cbor_hex"] == fixture["cbor_hex"]


def test_envelope_known_answers_match_rust_fixtures() -> None:
    envelopes = sorted((_JOURNAL_V1 / "envelope").glob("valid--*.json"))
    assert len(envelopes) == 39
    for path in envelopes:
        fixture = json.loads(path.read_text())
        answer = finstack_ai.journal_known_answer(
            "record_envelope", fixture["diagnostic_json"]
        )
        assert answer["payload_digest"] == fixture["payload_digest"]
        assert answer["checksum"] == fixture["checksum"]
        assert answer["cbor_hex"] == fixture["cbor_hex"]


def test_cost_amount_micros_stay_canonical_decimal_strings() -> None:
    fixture = json.loads(
        (_JOURNAL_V1 / "cbor-profile/valid--cost-amount-micros.json").read_text()
    )
    micros = fixture["diagnostic_json"]["micros"]
    assert micros == str(2**64 - 1)
    answer = finstack_ai.journal_known_answer(
        "record_body",
        {
            "session_created": {"metadata": {}},
        },
    )
    assert len(answer["payload_digest"]) == 64
