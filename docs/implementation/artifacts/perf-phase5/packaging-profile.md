# Phase 5 packaging-profile measurements

Date: 2026-08-17
Environment: Darwin 25.5.0 arm64; rustc 1.97.1 (`mise.toml` pin)
Machine-readable copy: [`packaging-profile-report.json`](packaging-profile-report.json)
Repeat command: `mise run measure-packaging-profile`

This note is host-local. It is not a 1.0 fail line and does not change
`[profile.release]`. Both built-in Python providers stay linked.

## Narrow mise tasks

These are local iteration gates. `mise run check` and `mise run ci` remain
the full workspace gates. Phase 1 `benchmark` / `benchmark-smoke` are
unchanged.

| Task | Scope |
| --- | --- |
| `mise run kernel` | `finstack-ai-kernel` clippy + test |
| `mise run runtime` | `finstack-ai-runtime` clippy (default and `native-tokio`) + test (`native-tokio`) |
| `mise run python-binding` | `finstack-ai-python` clippy + pytest |
| `mise run wasm-binding` | `finstack-ai-wasm` host clippy and `wasm32-unknown-unknown` check |

## Packaging profile (thin LTO + 16 codegen units)

Workspace default remains `lto = true`, `codegen-units = 1`, `strip = "symbols"`.
The probe used isolated `CARGO_TARGET_DIR` trees and:

- `CARGO_PROFILE_RELEASE_LTO=thin`
- `CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16`

| Artifact | default-release | thin-lto + 16 CGU | Notes |
| --- | --- | --- | --- |
| `libfinstack_ai_kernel.rlib` | 24,069,424 B / 12.9 s | 28,470,440 B / 6.7 s | rlib, not a wheel proxy |
| `libfinstack_ai_runtime.rlib` | 19,876,936 B / 19.7 s | 24,914,240 B / 10.3 s | rlib, not a wheel proxy |
| Python cdylib | see below | 16,719,840 B / 28.3 s | isolated thin-LTO rebuild |

Isolated default-release `finstack-ai-python` failed while compiling
`finstack-ai-runtime` (`error[E0624]: method note_head_checksum is private`
in `session_commit.rs`). That error is concurrent Phase 2 work, not a
packaging-profile defect. A pre-existing workspace
`target/release/lib_finstack_ai.dylib` from this host is 11,914,288 B; it
is not a clean paired rebuild.

Thin LTO compiled kernel/runtime faster and produced larger rlibs. The
isolated thin-LTO Python cdylib is about 40% larger than the pre-existing
default-release cdylib. A representative wheel is ~5.5 MiB against the
10 MiB budget. A 40% native-image increase would still likely fit, but
without a paired wheel and without Criterion on the alternate profile
that is not enough to change the default.

**Default `[profile.release]` is unchanged.**

### Deferred

- Full maturin wheel for each profile (plan estimate ~1m46s each; isolated
  default-release Python rebuild was already blocked).
- Criterion / NFR-PERF-001–003 re-bench against thin LTO. rlib size is not
  a runtime budget. Do not treat this note as permission to loosen
  NFR-PERF-001–007.

## Provider feature-gating

`linked_providers()` and `Agent.openai_compatible` / `Agent.anthropic` /
`Agent.ollama` are the public Python contract. Both HTTP providers stay
linked. No feature was removed.

Unique `cargo tree --edges normal` counts on this host:

| Target | unique names | unique packages |
| --- | --- | --- |
| openai-compatible | 149 | 195 |
| openai-compatible + `vendored-tls` | 149 | 195 |
| anthropic | 149 | 195 |
| anthropic + `vendored-tls` | 149 | 195 |
| `finstack-ai-python` (both providers, vendored) | 161 | 212 |
| `finstack-ai-server` | 114 | 148 |

`vendored-tls` vs default on either provider produced an empty package-set
diff, including `--edges normal,build`. On Darwin the graph is
`native-tls` + `security-framework`. `openssl-src` is absent from
`finstack-ai-python` even on build edges. The vendored OpenSSL compile
cost is a Linux/Windows wheel concern and was not re-measured here.

Dropping one provider was not built. It would shrink the Python graph but
would break `linked_providers()` and the factory methods. That needs an
explicit product decision.

## rustls vs vendored native TLS

Current Python wheel graph: `native-tls` present, `rustls` absent.
`finstack-ai-server` is the opposite (`rustls` present, `native-tls`
absent). Workspace `reqwest` stays
`["http2", "json", "native-tls", "stream"]`.

A live `cargo tree -p reqwest --features rustls` is rejected because
reqwest is not a workspace member. The isolated rustls switch was already
measured in [`../dep-graph/d1-tls-spike.md`](../dep-graph/d1-tls-spike.md):
**D2 = NO-GO**. Both reqwest 0.13 rustls feature sets fail `cargo-deny` on
`CDLA-Permissive-2.0` (`webpki-root-certs` via `rustls-platform-verifier`).
`deny.toml` still does not allow that license.

**Do not switch providers to rustls. Do not drop vendored-tls for Darwin
build speed.** Linux portable OpenSSL remains the reason the Python
binding enables `vendored-tls`.

## Verification

| Command | Result |
| --- | --- |
| `mise run kernel` | Pass |
| `mise run wasm-binding` | Pass |
| `mise run runtime` | Fail: concurrent Phase 2/4 worktree (`unused import: stream::AssembledToolTerminal`; coordinator test fixtures missing `TransitionEnv` / `KernelInput`) |
| `cargo clippy -p finstack-ai-python --all-targets --locked -- -D warnings` | Pass (lib only; crate has `test = false`) |
| `mise run python-binding` | Pytest not run; isolated default-release Python cdylib already failed compiling runtime |
| `mise tasks` | `benchmark` / `benchmark-smoke` still present |

`mise run check` and `mise run ci` were not rewritten.

## Disposition

- Default release profile: unchanged
- Built-in Python providers: unchanged
- rustls: not adopted
- Full wheel + NFR re-bench on thin LTO: deferred
- `runtime` / `python-binding` green runs wait on the concurrent runtime compile errors
