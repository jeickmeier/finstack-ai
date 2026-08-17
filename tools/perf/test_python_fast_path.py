"""Focused checks for rust-backed Python fast-path budget enforcement."""

from __future__ import annotations

import sys
from pathlib import Path

import pytest

sys.path.insert(0, str(Path(__file__).resolve().parent))

from python_fast_path import (
    EVENT_THROUGHPUT_TARGET,
    IDLE_SESSION_TARGET_BYTES,
    OVERHEAD_TARGET_PERCENT,
    WORKLOAD,
    _framework_owned_within_target,
    _validate_report,
)


def _report(
    *,
    overhead: float = 1.0,
    throughput_ok: bool = True,
    idle_ok: bool = True,
    ffi_batches: int = 4,
    model_deltas: int = 512,
    overlapped: bool = True,
) -> dict[str, object]:
    return {
        "format_version": 1,
        "workload": WORKLOAD,
        "commit": "test",
        "platform": {"system": "Darwin", "machine": "arm64", "release": "25.5.0"},
        "python": "3.14.0",
        "parameters": {"deltas_per_run": 512, "runs_per_sample": 64, "samples": 5},
        "timing_ns": {
            "import_median": 1,
            "construction_median": 1,
            "native_runs_median": 1,
            "python_runs_median": 1,
            "ffi_batch_conversion": 1,
            "external_io": 0,
        },
        "binding": {
            "overhead_percent": overhead,
            "target_percent": OVERHEAD_TARGET_PERCENT,
            "within_target": overhead <= OVERHEAD_TARGET_PERCENT,
        },
        "event_delivery": {
            "model_deltas": model_deltas,
            "logical_events": model_deltas,
            "ffi_batches": ffi_batches,
            "python_callbacks": 0,
            "serialized_bytes": 1,
            "batch_conversion_ns": 1,
            "rust_model_requests": 1,
        },
        "throughput": {
            "native_model_deltas_per_second": EVENT_THROUGHPUT_TARGET
            if throughput_ok
            else EVENT_THROUGHPUT_TARGET / 2.0,
            "python_model_deltas_per_second": EVENT_THROUGHPUT_TARGET,
            "target_model_deltas_per_second": EVENT_THROUGHPUT_TARGET,
            "native_within_target": throughput_ok,
        },
        "allocation": {
            "python_tracemalloc_peak_bytes": 1,
            "scope": "test",
        },
        "idle_memory": {
            "python_heap_bytes_per_idle_handle_pair": 1,
            "native_session_reference": {
                "bytes_per_session": 65_152,
                "framework_owned_bytes_per_session": 1_024
                if idle_ok
                else IDLE_SESSION_TARGET_BYTES + 1,
            },
            "target_bytes_per_session": IDLE_SESSION_TARGET_BYTES,
            "native_within_target": idle_ok,
        },
        "concurrency": {
            "entries_before_release": 2,
            "independent_runs_completed": 2,
            "overlapped": overlapped,
        },
        "external_io": {
            "measured_ns": 0,
            "included_in_binding_comparison": False,
            "scope": "test",
        },
    }


def test_validate_report_accepts_ratified_throughput_and_idle_flags() -> None:
    _validate_report(_report())


def test_validate_report_fails_when_throughput_misses_budget() -> None:
    with pytest.raises(RuntimeError, match="NFR-PERF-004"):
        _validate_report(_report(throughput_ok=False))


def test_validate_report_fails_when_idle_memory_misses_budget() -> None:
    with pytest.raises(RuntimeError, match="NFR-PERF-005"):
        _validate_report(_report(idle_ok=False))


def test_framework_owned_flag_ignores_rss_and_fails_closed_when_unmeasured() -> None:
    assert _framework_owned_within_target(
        {"bytes_per_session": 65_152, "framework_owned_bytes_per_session": 1_024}
    )
    assert not _framework_owned_within_target({"bytes_per_session": 1_024})
    assert not _framework_owned_within_target(
        {
            "bytes_per_session": 1_024,
            "framework_owned_bytes_per_session": IDLE_SESSION_TARGET_BYTES + 1,
        }
    )
