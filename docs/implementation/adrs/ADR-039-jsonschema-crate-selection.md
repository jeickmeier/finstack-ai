# ADR-039: jsonschema crate selection

## Status

Proposed

## Date

2026-08-17

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

ADR-022 keeps JSON Schema draft 2020-12 as the schema source of truth.
Technical Design §15.3 names `jsonschema` 0.40 as the default Rust/WASM
validator and requires an ADR with native/WASM conformance, size,
maintenance, and performance evidence before replacing it.

`finstack-ai-kernel` is 26 unique normal-graph packages.
`finstack-ai-runtime` is 97. The workspace `jsonschema` 0.40.2 subtree is
82 packages, including ICU data, `idna`, `fancy-regex`, and
`email_address`. Default features are already off. There is no feature
that drops format or ICU crates. The checked-in WASM artifact is
8,465,252 bytes against an 8,650,752-byte `wasm-bg` budget.

This record does not edit `docs/planning/` and does not supersede ADR-022.

Spike measurements and package lists live in
[artifacts/dep-graph/c1-jsonschema-spike.md](../artifacts/dep-graph/c1-jsonschema-spike.md).

## Decision

Keep `jsonschema` 0.40.2 (`default-features = false`) as the default
Rust/WASM tool-schema compiler. Do not swap the production validator.

The compile-once path in
`crates/finstack-ai-runtime/src/ports/tool/validator.rs` stays Draft
2020-12, offline `with_resources`, and `DenyRetriever`. Binding-native
validators remain fixture-parity peers. The kernel still imports no
validator.

## Consequences

Runtime and WASM unique-package counts stay at the measured baseline
(97 runtime host, 119 wasm host, 117 wasm `wasm32`). `deny.toml` keeps
`Unicode-3.0`, `MIT-0`, and `Zlib`. C2 (production swap) is not scheduled
by this record.

A later crate that keeps Draft 2020-12, offline `$ref`, a deny retriever,
and native plus `wasm32` support, and that actually drops the ICU/`idna`
cluster, can reopen the swap through a new ADR.

## Rejected alternatives

**`jsonschema` 0.49.9** (`default-features = false`). Isolated subtree
grows from 81 to 87 unique names (estimated runtime 97 → 103). It still
hard-depends on `idna`, `email_address`, `fancy-regex`, and ICU.
`with_resources` is removed in favor of an explicit `Registry`. Default
features add `resolve-http`, `resolve-file`, and `tls-aws-lc-rs` and are
rejected if left on.

**`boon` 0.6.1.** Draft 2020-12, `add_resource`, a deny `UrlLoader`, and
`wasm32` (with an explicit `getrandom` `wasm_js` pin) all work. Isolated
subtree is 55 unique names (estimated runtime 97 → 72). It still depends
on `idna = "1.0"` and the same ICU cluster, so WASM/wheel size does not
move. Last crates.io release is 2025-01-07. The default native loader
registers `file://` and must be replaced. Embedded metaschemas can grow
the WASM artifact. License allowlist entries cannot be removed from the
workspace: `Unicode-3.0` and `MIT-0` remain, and `Zlib` remains via
Wasmtime/`hashbrown`.

**Bounded portable-subset validator.** Last-resort analysis only. Current
tool-port fixtures use `type`, `properties`, `required`,
`additionalProperties`, `minimum`, and offline `$ref`. A fixture-only
engine could drop ICU and is the only measured path to a size-budget win,
but it would reject valid Draft 2020-12 schemas outside that keyword set
and would not be a conformant ADR-022 validator. Highest maintenance;
rejected unless a later ADR freezes a smaller dialect.

Any candidate that needs network retrieve, filesystem resolve, or a
different draft is rejected.

## Compatibility and schema-change classification

Implementation-crate selection only. JSON Schema family, fixtures, and
the `ToolValidator` / `ValidationIssue` shape stay as ADR-022 and TDD
§15.3 specify.

## Security classification

- References: TM-02, TM-16
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: No control change: offline retrieve denial,
  compile-once validation, and the 64-issue cap remain; no new trust
  boundary is introduced by keeping the current crate.

## Affected requirements, design, and delivery

- Affected requirements: FR-TLS, FR-SPEC
- Affected Technical Design: Technical Design §15.3 (JSON Schema 2020-12
  default Rust/WASM validator). TDD §15.3 still names `jsonschema` 0.40;
  this ADR records that the named crate remains the baseline. Planning
  files stay read-only.
- Architecture Specification §25 decision summary remains ADR-022
- Does not supersede ADR-022
- Mapped delivery: none while this record is Proposed. C2 is blocked
  until acceptance; the recommended accepted outcome schedules no swap.

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded. It
specializes ADR-022's implementation crate without changing the draft.

## Reconsideration conditions

May change only through a new superseding ADR. Reopening a production
swap requires a Draft 2020-12 crate that supports offline `$ref`, a deny
retriever, native and `wasm32-unknown-unknown` fixture parity, and a
measured drop of the ICU/`idna` cluster, plus size-budget and
`cargo-deny` evidence.

## Approval and implementation-evidence links

- Approval: pending (Proposed)
- Implementation evidence: Partial — C1 spike at
  [artifacts/dep-graph/c1-jsonschema-spike.md](../artifacts/dep-graph/c1-jsonschema-spike.md);
  no production `Cargo.toml` or validator swap
