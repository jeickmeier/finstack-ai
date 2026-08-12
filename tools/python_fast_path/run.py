#!/usr/bin/env python3
"""Measure the non-default Rust-backed Python fast-path fixture."""

from __future__ import annotations

import argparse
import asyncio
import json
import platform
import statistics
import subprocess
import sys
import time
import tracemalloc
from collections.abc import Sequence
from pathlib import Path
from typing import Any

REPO_ROOT = Path(__file__).resolve().parents[2]
DEFAULT_OUTPUT = REPO_ROOT / "target/python-fast-path/report.json"
WORKLOAD = "rust-backed-python-fast-path-v1"
OVERHEAD_TARGET_PERCENT = 10.0
EVENT_THROUGHPUT_TARGET = 100_000.0
IDLE_SESSION_TARGET_BYTES = 32 * 1024
COMPARISON_DELTAS = 512
COMPARISON_RUNS = 64
SMOKE_SAMPLES = 5
FULL_SAMPLES = 9


def _git_commit() -> str:
    completed = subprocess.run(
        ["git", "rev-parse", "HEAD"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    return completed.stdout.strip()


def _measure_import_ns(samples: int) -> int:
    code = (
        "import time; "
        "started=time.perf_counter_ns(); "
        "import finstack_ai; "
        "print(time.perf_counter_ns()-started)"
    )
    values = []
    for _ in range(samples):
        completed = subprocess.run(
            [sys.executable, "-c", code],
            cwd=REPO_ROOT,
            check=True,
            capture_output=True,
            text=True,
        )
        values.append(int(completed.stdout.strip()))
    return int(statistics.median(values))


async def _wait_for_gate(control: Any, name: str, entries: int) -> None:
    deadline = asyncio.get_running_loop().time() + 5.0
    while control.gate_entries(name) < entries:
        if asyncio.get_running_loop().time() >= deadline:
            raise RuntimeError(f"only {control.gate_entries(name)} runs reached {name}")
        await asyncio.sleep(0)


async def _await_run(agent: Any, input_text: str) -> Any:
    return await agent.run(input_text, max_cycles=1)


async def _measure_concurrency(native: Any) -> dict[str, int | bool]:
    gate = "python-independent-awaits"
    agent, control = await native._benchmark_agent(1, 2, gate)
    first = asyncio.create_task(_await_run(agent, "first independent run"))
    second = asyncio.create_task(_await_run(agent, "second independent run"))
    await _wait_for_gate(control, gate, 2)
    entries_before_release = control.gate_entries(gate)
    control.release(gate)
    results = await asyncio.gather(first, second)
    return {
        "entries_before_release": entries_before_release,
        "independent_runs_completed": len(results),
        "overlapped": entries_before_release == 2 and len(results) == 2,
    }


async def _measure_events(native: Any, deltas: int) -> dict[str, int]:
    agent, control = await native._benchmark_agent(deltas, 1)
    run = agent.start("event batching", max_cycles=1)
    batches = []

    async def collect() -> None:
        async for batch in run.events():
            batches.append(batch)

    collector = asyncio.create_task(collect())
    result = await run.result()
    await collector
    conversion_started = time.perf_counter_ns()
    encoded_bytes = sum(len(batch.to_json_bytes()) for batch in batches)
    conversion_ns = time.perf_counter_ns() - conversion_started
    logical_events = sum(len(batch.events()) for batch in batches)
    if len(result.text) != deltas * 8:
        raise RuntimeError("synthetic event workload returned unexpected text")
    return {
        "model_deltas": deltas,
        "logical_events": logical_events,
        "ffi_batches": len(batches),
        "python_callbacks": 0,
        "serialized_bytes": encoded_bytes,
        "batch_conversion_ns": conversion_ns,
        "rust_model_requests": control.request_count,
    }


async def _measure_construction(native: Any, samples: int) -> int:
    elapsed = []
    for _ in range(samples):
        started = time.perf_counter_ns()
        await native._benchmark_agent(1, 1)
        elapsed.append(time.perf_counter_ns() - started)
    return int(statistics.median(elapsed))


async def _measure_runs(
    native: Any, deltas: int, runs: int, samples: int
) -> tuple[int, int, int]:
    native_samples = []
    python_samples = []

    async def native_sample() -> tuple[int, int]:
        return await native._benchmark_native(deltas, runs)

    async def python_sample() -> int:
        agent, control = await native._benchmark_agent(deltas, runs)
        started = time.perf_counter_ns()
        for index in range(runs):
            result = await agent.run(f"python synthetic run {index}", max_cycles=1)
            if len(result.text) != deltas * 8:
                raise RuntimeError("synthetic Python workload returned unexpected text")
        elapsed = time.perf_counter_ns() - started
        if control.request_count != runs:
            raise RuntimeError(
                "synthetic Python workload issued an unexpected request count"
            )
        return elapsed

    await native._benchmark_native(deltas, 1)
    warm_agent, _ = await native._benchmark_agent(deltas, 1)
    await warm_agent.run("python synthetic warmup", max_cycles=1)
    native_deltas = 0
    for sample in range(samples):
        if sample % 2 == 0:
            native_elapsed, native_deltas = await native_sample()
            python_elapsed = await python_sample()
        else:
            python_elapsed = await python_sample()
            native_elapsed, native_deltas = await native_sample()
        native_samples.append(native_elapsed)
        python_samples.append(python_elapsed)
    expected_deltas = deltas * runs
    if native_deltas != expected_deltas:
        raise RuntimeError("native workload reported an unexpected delta count")
    return (
        int(statistics.median(native_samples)),
        int(statistics.median(python_samples)),
        expected_deltas,
    )


async def _measure_python_allocations(native: Any, deltas: int, runs: int) -> int:
    agent, _ = await native._benchmark_agent(deltas, runs)
    tracemalloc.start()
    try:
        for index in range(runs):
            await agent.run(f"allocation run {index}", max_cycles=1)
        _, peak = tracemalloc.get_traced_memory()
    finally:
        tracemalloc.stop()
    return peak


async def _measure_idle_python_heap(native: Any, handles: int) -> int:
    tracemalloc.start()
    try:
        before, _ = tracemalloc.get_traced_memory()
        retained = [await native._benchmark_agent(1, 1) for _ in range(handles)]
        after, _ = tracemalloc.get_traced_memory()
        if len(retained) != handles:
            raise RuntimeError("idle handle retention failed")
    finally:
        tracemalloc.stop()
    return max(0, after - before) // handles


def _native_idle_memory() -> dict[str, int] | None:
    if platform.system() == "Windows":
        return None
    completed = subprocess.run(
        [
            "cargo",
            "bench",
            "-p",
            "finstack-ai-test",
            "--bench",
            "native_runtime",
            "--locked",
            "--",
            "--sessions",
            "128",
        ],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
        text=True,
    )
    prefix = "IDLE_SESSION_MEMORY "
    matches = [
        line.removeprefix(prefix)
        for line in (completed.stdout + completed.stderr).splitlines()
        if line.startswith(prefix)
    ]
    if len(matches) != 1:
        raise RuntimeError("native idle-memory benchmark produced no unique marker")
    value = json.loads(matches[0])
    if not isinstance(value, dict):
        raise RuntimeError("native idle-memory benchmark returned invalid JSON")
    return {str(key): int(item) for key, item in value.items()}


def _validate_report(report: dict[str, object]) -> None:
    required = {
        "format_version",
        "workload",
        "commit",
        "platform",
        "python",
        "parameters",
        "timing_ns",
        "binding",
        "event_delivery",
        "throughput",
        "allocation",
        "idle_memory",
        "concurrency",
        "external_io",
    }
    if set(report) != required:
        raise RuntimeError(f"report keys differ: {sorted(set(report) ^ required)}")
    binding = report["binding"]
    events = report["event_delivery"]
    concurrency = report["concurrency"]
    if not isinstance(binding, dict) or not isinstance(events, dict):
        raise RuntimeError("report binding/event sections must be objects")
    if not isinstance(concurrency, dict):
        raise RuntimeError("report concurrency section must be an object")
    overhead_percent = float(binding["overhead_percent"])
    target_percent = float(binding["target_percent"])
    if overhead_percent > target_percent:
        raise RuntimeError(
            "Rust-backed Python workload exceeds the overhead target: "
            f"{overhead_percent:.3f}% > {target_percent:.3f}%"
        )
    if int(events["ffi_batches"]) >= int(events["model_deltas"]):
        raise RuntimeError("event delivery regressed to one crossing per model delta")
    if int(events["python_callbacks"]) != 0:
        raise RuntimeError("default Rust-backed delivery invoked Python callbacks")
    if concurrency.get("overlapped") is not True:
        raise RuntimeError("independent Rust runs serialized before the model gate")


async def _run(smoke: bool) -> dict[str, object]:
    import finstack_ai._finstack_ai as native

    deltas = COMPARISON_DELTAS
    runs = COMPARISON_RUNS
    samples = SMOKE_SAMPLES if smoke else FULL_SAMPLES
    import_ns = _measure_import_ns(samples)
    construction_ns = await _measure_construction(native, samples)
    native_ns, python_ns, total_deltas = await _measure_runs(
        native, deltas, runs, samples
    )
    events = await _measure_events(native, deltas)
    concurrency = await _measure_concurrency(native)
    allocation_peak = await _measure_python_allocations(native, deltas, runs)
    idle_python_heap = await _measure_idle_python_heap(native, 32)
    native_idle = _native_idle_memory()
    overhead_percent = ((python_ns / native_ns) - 1.0) * 100.0
    native_throughput = total_deltas * 1_000_000_000 / native_ns
    python_throughput = total_deltas * 1_000_000_000 / python_ns
    report: dict[str, object] = {
        "format_version": 1,
        "workload": WORKLOAD,
        "commit": _git_commit(),
        "platform": {
            "system": platform.system(),
            "machine": platform.machine(),
            "release": platform.release(),
        },
        "python": platform.python_version(),
        "parameters": {
            "deltas_per_run": deltas,
            "runs_per_sample": runs,
            "samples": samples,
        },
        "timing_ns": {
            "import_median": import_ns,
            "construction_median": construction_ns,
            "native_runs_median": native_ns,
            "python_runs_median": python_ns,
            "ffi_batch_conversion": events["batch_conversion_ns"],
            "external_io": 0,
        },
        "binding": {
            "overhead_percent": overhead_percent,
            "target_percent": OVERHEAD_TARGET_PERCENT,
            "within_target": overhead_percent <= OVERHEAD_TARGET_PERCENT,
        },
        "event_delivery": events,
        "throughput": {
            "native_model_deltas_per_second": native_throughput,
            "python_model_deltas_per_second": python_throughput,
            "target_model_deltas_per_second": EVENT_THROUGHPUT_TARGET,
            "native_within_target": native_throughput >= EVENT_THROUGHPUT_TARGET,
        },
        "allocation": {
            "python_tracemalloc_peak_bytes": allocation_peak,
            "scope": "Python frontend heap; Rust allocator excluded",
        },
        "idle_memory": {
            "python_heap_bytes_per_idle_handle_pair": idle_python_heap,
            "native_session_reference": native_idle,
            "target_bytes_per_session": IDLE_SESSION_TARGET_BYTES,
            "native_within_target": (
                None
                if native_idle is None
                else native_idle["bytes_per_session"] <= IDLE_SESSION_TARGET_BYTES
            ),
        },
        "concurrency": concurrency,
        "external_io": {
            "measured_ns": 0,
            "included_in_binding_comparison": False,
            "scope": "No network, provider, filesystem, or external store I/O",
        },
    }
    _validate_report(report)
    return report


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--smoke", action="store_true", help="run the bounded CI workload"
    )
    parser.add_argument("--output", type=Path, default=DEFAULT_OUTPUT)
    args = parser.parse_args(argv)
    report = asyncio.run(_run(args.smoke))
    args.output.parent.mkdir(parents=True, exist_ok=True)
    args.output.write_text(
        json.dumps(report, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(report, sort_keys=True))
    print(f"python-fast-path: wrote {args.output}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
