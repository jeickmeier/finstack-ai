---
trigger: glob
description: Enforce Rust production quality, safety, and rustdoc rules for finstack-ai crates.
globs: "**/*.rs"
---

# Rust coding

Apply with `01-engineering-conformance.md` and `02-testing-and-delivery.md`. Prefer the smallest current-scope change; do not reimplement binding or host concerns in Rust kernel/runtime code.

## Production-safe Rust

- Stable Rust is the production baseline. Follow the repository toolchain, formatter, Clippy, feature, documentation, and minimum-version policies once their configuration exists.
- Recoverable input, provider, extension, storage, protocol, and configuration failures return stable errors. Do not leak secrets in errors, events, logs, or default diagnostics.
- Preserve error code, category, and retryability across bindings. Keep policy denial, validation retry, cancellation, deadline, and uncertain outcome distinct. Local source chains are diagnostic only and are never persisted or hashed.
- Avoid `unwrap`, `expect`, indexing assumptions, and integer-conversion panics on public or external input paths. New `unsafe` requires an adjacent safety rationale, focused invariant tests, and the required reviewer.
- Give authoritative mutable state one owner. Do not hold locks across external I/O or host-language callbacks.
- Bound queues, streams, batches, payloads, retries, collections, concurrency, and retained output before resource commitment. Every spawned task has cancellation, shutdown, and join/detach behavior.
- Pass semantic time and randomness explicitly. Preserve typed identifier, canonical JSON/CBOR, digest, ordering, and compatibility rules.
- Use typed lowercase UUID values with UUIDv7 for newly allocated runtime entities, bounded strict `RawJson`, shared immutable byte buffers, stable error codes, and source-free serializable error descriptors at durable or cross-language boundaries.
- Native port objects remain `Send + Sync`; browser-WASM host objects remain local and must not be falsely marked thread-safe. Tokio and native adapter dependencies cannot leak into browser builds.
- Release the Python GIL for Rust-only work. Python callbacks remain coarse, cancellable, and timeout-bounded.

## rustdoc

Public and compatibility-controlled APIs require documentation that states purpose, failure semantics, and a tested example where the surface is user-facing. Private helpers need docs only when behavior is non-obvious. Keep comments current with code; delete stale commentary.

- Use `///` on public items and `//!` for module-level docs. Document invariants, ownership/borrowing expectations, and feature-gate differences that affect callers.
- Include `# Errors` for fallible public functions, `# Panics` when a public path can panic, and `# Safety` plus adjacent invariant notes for every `unsafe` API or block.
- Prefer rustdoc examples that compile under documentation tests for primary public constructors and workflows. Link related types rather than duplicating contracts.
