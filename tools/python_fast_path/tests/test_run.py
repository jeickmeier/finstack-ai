from __future__ import annotations

import importlib.util
from pathlib import Path

import pytest

RUN_PATH = Path(__file__).resolve().parents[1] / "run.py"
SPEC = importlib.util.spec_from_file_location("python_fast_path_run", RUN_PATH)
assert SPEC and SPEC.loader
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


def _report() -> dict[str, object]:
    return {
        "format_version": 1,
        "workload": runner.WORKLOAD,
        "commit": "0123456789abcdef",
        "platform": {"system": "Darwin", "machine": "arm64", "release": "1"},
        "python": "3.14.0",
        "parameters": {"deltas_per_run": 128, "runs_per_sample": 4, "samples": 3},
        "timing_ns": {
            "import_median": 1,
            "construction_median": 1,
            "native_runs_median": 100,
            "python_runs_median": 105,
            "ffi_batch_conversion": 1,
            "external_io": 0,
        },
        "binding": {
            "overhead_percent": 5.0,
            "target_percent": 10.0,
            "within_target": True,
        },
        "event_delivery": {
            "model_deltas": 128,
            "logical_events": 130,
            "ffi_batches": 5,
            "python_callbacks": 0,
            "serialized_bytes": 1024,
            "batch_conversion_ns": 1,
            "rust_model_requests": 1,
        },
        "throughput": {
            "native_model_deltas_per_second": 1.0,
            "python_model_deltas_per_second": 1.0,
            "target_model_deltas_per_second": 100000.0,
            "native_within_target": False,
        },
        "allocation": {
            "python_tracemalloc_peak_bytes": 1,
            "scope": "Python frontend heap; Rust allocator excluded",
        },
        "idle_memory": {
            "python_heap_bytes_per_idle_handle_pair": 1,
            "native_session_reference": None,
            "target_bytes_per_session": 32768,
            "native_within_target": None,
        },
        "concurrency": {
            "entries_before_release": 2,
            "independent_runs_completed": 2,
            "overlapped": True,
        },
        "external_io": {
            "measured_ns": 0,
            "included_in_binding_comparison": False,
            "scope": "none",
        },
    }


def test_report_accepts_batched_in_target_overlap() -> None:
    runner._validate_report(_report())


@pytest.mark.parametrize("section", ["binding", "event_delivery", "concurrency"])
def test_report_rejects_acceptance_regressions(section: str) -> None:
    report = _report()
    if section == "binding":
        report["binding"]["overhead_percent"] = 10.1  # type: ignore[index]
    elif section == "event_delivery":
        report["event_delivery"]["ffi_batches"] = 128  # type: ignore[index]
    else:
        report["concurrency"]["overlapped"] = False  # type: ignore[index]
    with pytest.raises(RuntimeError):
        runner._validate_report(report)
