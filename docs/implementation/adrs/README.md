# ADR record directory

This is the canonical location for standalone ADR records. Accepted ADR-001
through ADR-038, Proposed ADR-039, and Accepted ADR-040 through ADR-045
are indexed in the [ADR database](../adr-register.md). ADR-001 through
ADR-037 were materialized here as versioned standalone records under
logical PR-004; ADR-038 through ADR-045 are later implementation records.
ADR-040 supersedes ADR-023.

Files use `ADR-NNN-short-topic.md`. The ADR database must link each file before
its record state is `Standalone`; each file must satisfy the standalone-record
requirements in that database.

Do not create a second decision by copying planning prose here. A standalone
record captures the stable decision context, consequences, change-control
metadata, security and compatibility classification, and links to the canonical
planning sections. New or superseding decisions follow the planning authority
chain and receive the next approved ADR number.

| ADR | File |
| --- | --- |
| ADR-001 | [ADR-001-microkernel-boundary.md](ADR-001-microkernel-boundary.md) |
| ADR-002 | [ADR-002-kernel-continuation.md](ADR-002-kernel-continuation.md) |
| ADR-003 | [ADR-003-deterministic-effects.md](ADR-003-deterministic-effects.md) |
| ADR-004 | [ADR-004-commit-before-effect.md](ADR-004-commit-before-effect.md) |
| ADR-005 | [ADR-005-six-ports.md](ADR-005-six-ports.md) |
| ADR-006 | [ADR-006-direct-native-path.md](ADR-006-direct-native-path.md) |
| ADR-007 | [ADR-007-shared-binding-engine.md](ADR-007-shared-binding-engine.md) |
| ADR-008 | [ADR-008-declarative-capabilities.md](ADR-008-declarative-capabilities.md) |
| ADR-009 | [ADR-009-observer-middleware-separation.md](ADR-009-observer-middleware-separation.md) |
| ADR-010 | [ADR-010-optional-wasm-isolation.md](ADR-010-optional-wasm-isolation.md) |
| ADR-011 | [ADR-011-no-native-dylib-abi.md](ADR-011-no-native-dylib-abi.md) |
| ADR-012 | [ADR-012-immutable-lanes.md](ADR-012-immutable-lanes.md) |
| ADR-013 | [ADR-013-at-least-once-effects.md](ADR-013-at-least-once-effects.md) |
| ADR-014 | [ADR-014-protocol-separation.md](ADR-014-protocol-separation.md) |
| ADR-015 | [ADR-015-canonical-cbor.md](ADR-015-canonical-cbor.md) |
| ADR-016 | [ADR-016-sqlite-before-multilane.md](ADR-016-sqlite-before-multilane.md) |
| ADR-017 | [ADR-017-single-python-wheel.md](ADR-017-single-python-wheel.md) |
| ADR-018 | [ADR-018-python-version-matrix.md](ADR-018-python-version-matrix.md) |
| ADR-019 | [ADR-019-browser-host-adapter.md](ADR-019-browser-host-adapter.md) |
| ADR-020 | [ADR-020-capability-delivery.md](ADR-020-capability-delivery.md) |
| ADR-021 | [ADR-021-shared-framing.md](ADR-021-shared-framing.md) |
| ADR-022 | [ADR-022-json-schema-2020-12.md](ADR-022-json-schema-2020-12.md) |
| ADR-023 | [ADR-023-reference-provider.md](ADR-023-reference-provider.md) |
| ADR-024 | [ADR-024-governance-and-license.md](ADR-024-governance-and-license.md) |
| ADR-025 | [ADR-025-generic-deferred-effects.md](ADR-025-generic-deferred-effects.md) |
| ADR-026 | [ADR-026-run-lineage.md](ADR-026-run-lineage.md) |
| ADR-027 | [ADR-027-typed-interactions.md](ADR-027-typed-interactions.md) |
| ADR-028 | [ADR-028-before-finalize.md](ADR-028-before-finalize.md) |
| ADR-029 | [ADR-029-typed-identifiers.md](ADR-029-typed-identifiers.md) |
| ADR-030 | [ADR-030-boxed-port-abi.md](ADR-030-boxed-port-abi.md) |
| ADR-031 | [ADR-031-worker-based-wasm.md](ADR-031-worker-based-wasm.md) |
| ADR-032 | [ADR-032-disposable-snapshots.md](ADR-032-disposable-snapshots.md) |
| ADR-033 | [ADR-033-explicit-interruption.md](ADR-033-explicit-interruption.md) |
| ADR-034 | [ADR-034-durable-middleware-outcomes.md](ADR-034-durable-middleware-outcomes.md) |
| ADR-035 | [ADR-035-experimental-wit-versioning.md](ADR-035-experimental-wit-versioning.md) |
| ADR-036 | [ADR-036-blob-storage-boundary.md](ADR-036-blob-storage-boundary.md) |
| ADR-037 | [ADR-037-middleware-compaction.md](ADR-037-middleware-compaction.md) |
| ADR-038 | [ADR-038-ciborium-test-interop.md](ADR-038-ciborium-test-interop.md) |
| ADR-039 | [ADR-039-jsonschema-crate-selection.md](ADR-039-jsonschema-crate-selection.md) |
| ADR-040 | [ADR-040-openai-responses-native-ollama.md](ADR-040-openai-responses-native-ollama.md) |
| ADR-041 | [ADR-041-mid-run-capability-activation-and-variants.md](ADR-041-mid-run-capability-activation-and-variants.md) |
| ADR-042 | [ADR-042-model-assisted-compaction-runtime-phase.md](ADR-042-model-assisted-compaction-runtime-phase.md) |
| ADR-043 | [ADR-043-process-confinement-backends.md](ADR-043-process-confinement-backends.md) |
| ADR-044 | [ADR-044-skills-first-load-time-import.md](ADR-044-skills-first-load-time-import.md) |
| ADR-045 | [ADR-045-first-class-sdk-constructors.md](ADR-045-first-class-sdk-constructors.md) |
