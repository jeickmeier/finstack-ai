# PR-063 flamegraph stand-in

Criterion HTML under `target/criterion` contains absolute host paths and is
not checked in (TM-04). This file is the published representative profile.

Host: Darwin 25.5.0 arm64; rustc 1.97.1; gate 100 samples / 5 s / 1 s warmup.

## Hot paths that stay in budget

| Frame | Typical median | Notes |
| --- | --- | --- |
| `Kernel::default` | ~157 ns | NFR-PERF-006 kernel init |
| `Kernel::decide(AcceptRun)` | ~163 ns | NFR-PERF-001 |
| `Kernel::apply(AcceptRun)` | measured in `kernel_micro` | TDD §33.1 apply |
| `InstantModel::request` poll | NFR-PERF-002 | boxed port future; no registry lookup |
| `RawJson::parse` small object | ~607 ns | construction, not per-token |
| `execute_model_only_completion` | ~165 µs | includes scripted model + reducer |
| `assemble_256_text_items` | PR-020 ~14.5 µs / stream | NFR-PERF-004 >> 100k events/s |

## What was not on the hot path

- Registry lookup after resolve (ENG-ARCH-004). `resolved_handle_descriptor`
  is a direct `Arc<dyn Model>` call.
- Schema/validator compile after construction (NFR-PERF-007).
- Provider, network, or durable-store I/O.

## Tuning that would need an ADR

Skipping checksums, skipping commit-before-effect, unbounding queues,
per-token binding callbacks, or unboxing public port futures (ADR-030).
None of those were required by the measured numbers.
