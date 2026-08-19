# finstack-ai-kernel conformance and correctness review

Status: independent review of `crates/finstack-ai-kernel` at version 1.0.0, commit `cae7b91`.

Authority: this file records review findings only. It does not amend `docs/planning/`,
does not claim gate evidence, and does not assert ownership, approval, or completion for
any logical PR. Section references (§) are to
`docs/planning/03-finstack-ai-technical-design.md`.

Scope: conformance against the Technical Design Document, correctness of the reducer and
state machine, completeness of declared features, and a sweep for unfinished or
decorative code.

## Ground truth at review time

| Check | Result |
| --- | --- |
| `cargo test -p finstack-ai-kernel` | 106 unit + 101 integration + 1 semantic reference + 70 doctests + 2 others; all passing, 1 ignored (a fixture printer) |
| `cargo clippy -p finstack-ai-kernel --all-targets` | 0 warnings under `clippy::all` + `clippy::pedantic` + `missing_docs` |
| Working tree | clean at `cae7b91` |

Findings recorded below: 10 high (F1–F10), 15 medium (F11–F25), 12 low (F26–F37).
F3 and F8 each merge two independently reported findings of the same defect.

## Verdict

This is a well-built kernel and the engineering discipline behind it is high. The
determinism story is real and enforced structurally; the error model matches the spec
code-for-code; the panic surface is essentially nil; and the settlement-fingerprint layer
— the most subtle part of the design — is implemented as specified, with dedicated
explicit-null projection DTOs everywhere the spec demands them.

The defects cluster in one recognisable place: **the layers added last**. Kernel-state
versions 4, 5 and 6 repeatedly break patterns that versions 1–3 establish correctly —
monotonic version promotion becomes plain assignment, `>=` validation gates become `==`,
projection DTOs give way to raw human-readable structs. Separately, two content types
cannot survive a canonical-CBOR round trip, which breaks durable replay for any message
carrying a tool result.

Features are substantially complete for the current scope. The "reserved for later PR"
markers that were checked are honest deferrals with real owners elsewhere in the
workspace, not stubs. One exception is a genuine no-op enum arm (F13).

## What is solid

Verified directly against source:

- **Determinism is structurally enforced.** Zero `HashMap` or `HashSet` in the entire
  crate — every keyed collection is a `BTreeMap`/`BTreeSet`, so hash projections and
  validation sets iterate in a defined order. No float, no address-dependent behaviour.
- **The panic surface is effectively zero.** Four `unreachable!` sites in non-test code,
  each provably dead behind an earlier guard; no `todo!`, no `unimplemented!`, no
  reachable `unwrap`. Consistent with §30.3.
- **The error model is exact.** All 29 `KernelError` variants from §11.3.5, exact
  `snake_case` codes, no extras, and every variant is constructed in non-test code.
- **Settlement fingerprints are correct.** Every nested struct with optional fields gets a
  dedicated recursive-explicit-null projection (`ErrorProjection`, `MessageProjection`,
  `UsageProjection`, `ToolResultFingerprintV1`). The raw DTOs that *are* embedded carry no
  omission-bearing serde attributes.
- **Tool execution grouping matches §21.2 exactly**, including first-call-`Sequential`,
  all-`Barrier` and single-call edges, with a `checked_add` on the group counter.
- **No dead surface.** All 18 `RunPhase` values and all 14 `KernelInput` variants are
  handled, with no catch-all arm.

## High severity

### F1 — Tool results cannot be decoded from canonical CBOR

`crates/finstack-ai-kernel/src/content/content_block.rs:172` — `ToolResultContentItems::deserialize`

`ToolResultContentItems::deserialize` branches on `deserializer.is_human_readable()`, and
the human branch reads each element as `Box<serde_json::value::RawValue>`. But
`ContentBlock` decodes through the internally tagged `BinaryContentBlock`, and serde
implements internal tagging by buffering into `ContentDeserializer` — whose
`is_human_readable()` returns `true` even on CBOR. The human visitor is therefore selected
on a binary payload and `RawValue` decoding fails.

Reproduced end to end: encoding `ContentBlock::ToolResult` with one text block via
`finstack_ai_protocol::encode` and decoding it back yields
`Err(Codec { message: "invalid type: newtype struct, expected any valid JSON value" })`.
Same for `Vec<ContentBlock>` — the shape of `Message.content`. Empty tool-result content
decodes fine, because the loop returns before touching an element, which is why the
existing tests miss it. Decoding a bare `ToolResultBlock` outside the tagged enum also
succeeds, confirming the tagging path is the trigger.

Impact: any durable journal record or snapshot containing a tool-result message cannot be
restored.

Fix: drop the `is_human_readable()` dispatch; use one visitor that reads elements as
`ContentBlock` and rejects nested tool blocks after decoding. Add a CBOR round-trip
fixture with non-empty content.

### F2 — Opaque content cannot be decoded from canonical CBOR

`crates/finstack-ai-kernel/src/content/opaque.rs:171` — `OpaquePayload::deserialize`

Identical root cause, second site. The binary encoder writes `encoding` plus `data` as a
byte string; the human branch expects `data_hex`/`data` through `RawValue`. Both payload
variants fail, not just the bytes one — the `json` variant produces the same error.

The crate already documents this exact serde quirk at
`crates/finstack-ai-kernel/src/primitives/bounds.rs:80` and works around it there for
`BoundedString`, so the pattern was known and simply not applied to these two types.

### F3 — `state_version` is assigned, not raised, so later records demote a higher-version state

`crates/finstack-ai-kernel/src/reducer/apply/record.rs:287, 298, 344, 375, 397, 420, 445`
and `crates/finstack-ai-kernel/src/reducer/apply/output.rs:65, 134`

Kernel-state promotion is one-way, and nearly every apply site honours it with
`state.state_version = state.state_version.max(N)`. Nine sites instead use plain
assignment: four `= 4` and five `= 5`.

Concrete failure: a run promoted to v6 by an approval interaction, or to v5 by a child-run
preparation, then records a structured final result and is demoted to v4.
`KernelState::validate` rejects any state whose version is below the features it holds, and
`state_hash` dispatches a different projection per version. The kernel therefore emits
records it then refuses to apply.

Fix: use `.max(4)` / `.max(5)` at all nine sites. Add a reducer test that applies an
interaction record followed by a `FinalResultRecorded` record and asserts the resulting
`state_version` is 6.

### F4 — v4 structured-output invariants are silently skipped at v5 and v6

`crates/finstack-ai-kernel/src/state/validate.rs:318` — `if self.state_version == 4`

Every other version gate in `KernelState::validate` is a `>=` gate (lines 119 and 238),
because promotion is monotone. The PR-012 block alone uses `==`, so at state versions 5 and
6 the whole structured-output invariant set is skipped: `output_configuration.validate()`,
the `active_capabilities` strict-sort/uniqueness check, the `final_result` /
`validation_failure` mutual exclusion, the terminal-candidate and configuration
cross-checks, and the missing-configuration check.

Structured-output state is fully reachable at v5 and v6, so this is live.

Fix: change to `>= 4`, matching the `>= 3` and `>= 5` gates, and run the existing v4
rejection cases at state versions 4, 5 and 6.

### F5 — v1/v2 snapshot wire silently drops `accepted_at` and `limit_usage`

`crates/finstack-ai-kernel/src/state/wire.rs:34` — `KernelStateWireV1` / `KernelStateWireV2`

Neither wire struct carries `accepted_at` or `limit_usage`, and deserialization maps the
absent fields to `None` / `LimitUsage::default()`. But the reducer populates both while the
state is still v1: `apply_record` sets `accepted_at` on `RunAccepted`
(`reducer/apply/record.rs:55`) and increments `limit_usage.turns` / `.context_bytes` on
`ContextPrepared` (`reducer/apply/record.rs:78-89`), neither of which bumps the version.
`validate()`'s `has_control_state` deliberately omits both fields, so such a state is a
legal v1 state.

The v1 state hash excludes both fields, so a snapshot/restore cycle loses accumulated
wall-time and counter state **without the hash detecting it**. Later limit evaluation then
runs against reset counters.

Note the fix cannot simply add the fields to the v1 wire: §11.3.6.1 freezes the v1 hash and
§22.6.4 requires v1/v2 to reject v3 fields. Either stop populating both fields until the
state reaches v3 (deriving them from the replayed prefix), or carry them across the v1/v2
wire under names the v1/v2 hash still excludes.

### F6 — A child run may reuse its parent's run identity

`crates/finstack-ai-kernel/src/run/accepted.rs:89` — `RunAccepted::try_new`

For a non-root run, every lineage check is delegated to `validate_child_against_parent`,
which is never given the child's own `run_id` and so cannot compare it to the parent's.
`validate_against_parent` (`accepted.rs:157`) does have `self.run_id` in hand but never
compares it either.

Result: `RunAccepted::try_new(parent.run_id(), child_relation, …, Some(&parent))` succeeds,
producing a run that is its own parent — `relation.parent_run_id() == Some(run_id)` with
`kind != Root` and `depth == 1`. The error variant for this already exists and is never
raised.

Fix: thread `run_id` into the child validator and reject with
`RunError::ChildRunIdentityReuse`.

### F7 — A second tool deferral in one execution group: `decide` accepts what `apply` rejects

`crates/finstack-ai-kernel/src/reducer/apply/shapes.rs:71` — `validate_batch_shape`

`validate_batch_shape` passes `allow_deferred = false` for `AwaitingExternal` but `true`
for `AwaitingTools` (line 90), and the `EffectDeferred` arm is gated on that flag
(line 408). So once *any* call in a batch defers, the phase becomes `AwaitingExternal` and
no further deferral can be applied. But `decide` explicitly accepts a direct
`ToolBatchSettled` in both phases (`reducer/tool/decide_settle.rs:201`) and will emit a
second `EffectDeferred` for a different call.

A parallel group with two tools that both defer therefore produces a decision the kernel
then refuses to apply — `invalid_record_order` on a legal sequence. §11.2 states the phase
covers "one or more deferred tool effects", plural.

Fix: make the `allow_deferred` gate call-scoped rather than phase-scoped — accept an
`EffectDeferred` whenever the targeted call is `Requested { deferred: None }` and its
`group_index` is the current group, independent of other calls.

### F8 — `EffectCancelled` events bypass the sensitivity and correlation policy

`crates/finstack-ai-kernel/src/events/derive.rs:63` and `:87` — `validate_event_policy`

The `model_event` and `tool_event` classifiers match
`EffectRequested | EffectDeferred | EffectCompleted | EffectFailed` plus the transient
deltas, but omit `EffectCancelled` from both, so it falls through `_ => false` and no
correlation or sensitivity rule applies. The sibling record classifiers in the same file,
`is_model_effect_record` and `is_tool_effect_record`, do include it.

The record-derived path stamps `Sensitivity::Confidential` on a cancelled model effect,
while the public `RunEvent::try_durable` / `try_transient` constructors accept the same body
as `Sensitivity::Public` with `turn_id = None` and `model_request_id = None`. Two
independent reviewers found this; both were confirmed, one with a reproduction. This is the
one finding with a plausible confidentiality consequence.

Fix: add
`RunEventBody::EffectCancelled(cancelled) => cancelled.output_contract().kind == EffectOutputKind::ModelResponse`
to the `model_event` match and the `ToolResult` equivalent to `tool_event`, mirroring the
record classifiers. Add a test asserting a model/tool `EffectCancelled` event is rejected
when published as `Public` without correlations.

### F9 — Limit observation runs before duplicate classification (contested)

`crates/finstack-ai-kernel/src/reducer/decide/mod.rs:58`

`decide` calls `decide_limit` before dispatching to the per-input handlers that classify a
settlement as an equal post-commit duplicate. The verifier reproduced a case where an exact
redelivery of an already-committed input is counted again against a limit dimension and can
terminally fail the run. §22.6.2 defines every limit dimension as a *replay-derived*
observation, which a redelivery should not move.

**This one needs maintainer judgment.** A closely adjacent claim about retry counting on
the same line was *refuted* by a different verifier, so the two analyses disagree about
scope. The ordering fact is not in dispute; what a redelivery actually does to each
dimension should be confirmed before changing anything.

### F10 — No oracle pins the kernel-state hash above version 1

`crates/finstack-ai-kernel/tests/model_only_reducer/tool_batches/mod.rs:363` (also
`termination/mod.rs:134`, `structured_output.rs:67`)

The only assertions over the state hash at v2, v3 and v4 compare
`replayed.state().state_hash()` against `state.state_hash()` — the implementation against
itself. The recorded fixtures store only the resulting boolean. There is no byte-exact
known-answer fixture for any version above 1.

The state hash is the compatibility surface for snapshots and cross-implementation
agreement. As written, a change to any v2–v6 projection changes the hash for every existing
run and no test fails. This is the single highest-leverage gap in the test suite, and it is
why F3, F4 and F5 could all land undetected.

## Medium severity

### F11 — State-hash projection embeds raw human-readable DTOs

`crates/finstack-ai-kernel/src/state/hash_projection/schema.rs:173, 256, 258-262, 329-331, 363-365`

§11.3.6 is explicit: "Implementations must not serialize ordinary human-readable DTOs
directly where their Serde form omits absent optionals." The crate follows this rigorously
for v1–v3 and for every fingerprint — `ErrorProjection`, `LimitUsageProjection` and
`RunLimitsProjection` exist for exactly this reason. The v4/v5/v6 additions bypass them.

Two examples make the inconsistency self-evident:

- `last_limit: Option<&LimitReached>` carries a raw `LimitUsage`, bypassing the
  `LimitUsageProjection` used two fields earlier for `state.limit_usage`.
- `validation_failure` carries a raw `ErrorDescriptor`, whose `ErrorIdentifiers` has
  thirteen `skip_serializing_if` fields — the same type that gets `ErrorProjection` inside
  `StageFailFingerprintV1`.

Same for `child_preparations`, `budget_reservations`, `budget_charges`,
`pending_interaction` and `last_interaction_terminal`.

The hazard is that the canonical hash is now coupled to a presentational serde choice:
removing a `skip_serializing_if` for API reasons silently changes the hash of every affected
run, which is precisely what the recursive-explicit-null rule exists to prevent.

### F12 — `AssigneeHint` decode/encode asymmetry

`crates/finstack-ai-kernel/src/refs/identity.rs:259` and
`crates/finstack-ai-kernel/src/effects/interaction.rs:135`

`Deserialize` bounds `Role`/`Queue` with `BoundedString<LABEL_MAX_BYTES>`, which checks
length only. `Serialize` validates with `label_is_valid`, which additionally requires
non-empty and no NUL byte. So `{"role":""}`, and any role or queue label containing an
escaped NUL, decode successfully but cannot be re-encoded.

A journal record carrying such a value restores into `KernelState`, and the first
`state_hash()` or interaction fingerprint fails — turning a value that should have been
rejected at decode into a `StateHashFailed` on a live run. §6.5 requires decoders to check
the applicable limits and return a stable error. `InteractionRequest::try_new` also skips
the check (it validates `InteractionKind::Custom` but not the assignee hint), so it fails
late rather than at the validating constructor.

### F13 — `UnknownUsagePolicy::AllowWithinReservedMaximum` is an unimplemented no-op

`crates/finstack-ai-kernel/src/reducer/decide/limit.rs:83`

The match arm is literally `=> {}`, while its two siblings (`FailClosed` at lines 56-64,
`SuspendForDecision` at 65-82) do real work, and the arm is reached in exactly the spec's
scenario. This is the one place where a declared feature is genuinely not implemented.

### F14 — `RawJson` materializes the whole value before applying its byte ceiling

`crates/finstack-ai-kernel/src/primitives/raw_json.rs:235` — `visit_seq` / `visit_map`

Both visitors build a full `serde_json::Value` and only then call `RawJson::parse`, so the
`RAW_JSON_MAX_BYTES` check sees re-serialized text after allocation. §6.5 requires decoders
to check byte, item and depth limits *before allocation*. Scoped to the human-readable JSON
path — `visit_bytes`/`visit_byte_buf` pass input straight through, so the durable
canonical-CBOR path is unaffected.

### F15 — `Metadata` applies `RawJson`'s limits instead of its own

`crates/finstack-ai-kernel/src/primitives/raw_json.rs:365-373`

`impl Deserialize for Metadata` routes through `RawJson`'s 1 MiB / depth-32 ceilings before
applying `Metadata`'s own much tighter 64 KiB / depth-16 / 64-member limits, so a value that
is oversized for `Metadata` is fully materialized first.

### F16 — Tool batch record arrays decode without the §6.5 item ceiling

`crates/finstack-ai-kernel/src/tools/types.rs:157` — `ToolBatchOpened.calls`

`Arc<[AssignedToolCall]>` under a plain `#[derive(Deserialize)]` with no `BoundedVec`.
Verification corrected the blast radius: this is **not** unbounded end to end —
`reducer/apply/tools.rs:38` rejects `opened.calls.len() > SEMANTIC_ARRAY_MAX_ITEMS` and
`reducer/input.rs:235` bounds the command-side plan array. It remains a
decode-before-allocation gap on the record path.

### F17 — Tool-batch opening validates allocated IDs before the capacity preflight

`crates/finstack-ai-kernel/src/reducer/tool/decide_open.rs:58`

`validate_allocated_ids` runs at line 58 while `capacity::preflight_decision` only appears
at lines 137-145 — inverting the normative decide order, which puts capacity first so a
capacity failure reports `state_capacity_exceeded` rather than an ID error.

### F18 — `RecordBody::RetryScheduled` escapes `ErrorDescriptor` re-validation

`crates/finstack-ai-kernel/src/records/validate.rs:147`

`validate_body_for_creation` collects an `&ErrorDescriptor` from every other failure-bearing
body — `StageOutcomeRecorded { disposition: Failed }`, `EffectFailed`, `ToolCallSettled`,
`ToolBatchClosed { outcome: Failed }`, `RunFailed` — but not from
`RetryScheduled.prior_error` (`lifecycle/mod.rs:219`).

### F19 — `ToolProgress` and `ProviderHeartbeat` are write-only public types

`crates/finstack-ai-kernel/src/events/transient.rs:129-165` and `:195-225`

Both have private fields, a `try_new`, and no accessors — so a consumer of the public API
can construct them but never read `message`, `percent`, `provider` or `detail`. For a
published event vocabulary this makes the bodies unusable by observers.

### F20 — `semantic_reference.rs` cannot detect reference drift

`crates/finstack-ai-kernel/tests/semantic_reference.rs:11`

The single vocabulary assertion iterates a hand-maintained `required` array rather than the
enums themselves, so a new variant is never detected as undocumented. Verification counted
**14** public variants currently missing from the reference: three `KernelInput`
(`RecordExternalCommandRejected`, `RequestInteraction`, `InteractionSettled`) and eleven
`RecordBody`.

### F21 — Four mandatory §11.3.5 error-code rows have no asserting test

`crates/finstack-ai-kernel/src/reducer/decide/model.rs:113` and siblings

`assistant_message_presence_mismatch`, `effect_not_pending`,
`model_request_contract_mismatch` and `duplicate_tool_call` are all raised in source but no
test asserts the specific code. §11.3.5 mandates the failure-to-code mapping, not literally
one test per row, so read this as a coverage gap rather than a spec violation.

### F22 — `derived_event_kind` is public API with no direct tests

`crates/finstack-ai-kernel/src/events/derive.rs:394`

The §20.2.1 ordinal table is exercised only indirectly and the `kind_version` guard is
unexercised. Since the ordinal table is a wire-compatibility surface, it warrants a direct
table-driven test.

**Current status (2026-08-18): fixed.** `derived_event_kind` is crate-internal and
`derived_event_kind_matches_section_20_2_1_ordinals` directly covers the ordinal table,
unsupported kind versions, and unsupported ordinals.

### F23 — `state_hash` rustdoc claims schema-1 while the method dispatches six schemas

`crates/finstack-ai-kernel/src/state/mod.rs:194`

"Compute the exact schema-1 JCS state digest under `kernel-state`" sits directly above a
body that builds `DigestWriter::new("kernel-state", u32::from(self.state_version))` and
branches over versions 1–6. Documentation only, but on the crate's most
compatibility-critical method.

**Current status (2026-08-18): implementation documentation fixed.** The live rustdoc now
describes a versioned V1–V6 projection. Planning §11.3.6 still describes schema V1 and
requires separate change control; this review does not amend the planning contract.

### F24 — A stale test passes for the wrong reason

`crates/finstack-ai-kernel/tests/model_only_reducer/apply_and_json/decode_bounds.rs:23`

`terminal_state_vocabulary_is_exactly_completed_and_failed` still asserts a two-variant
vocabulary, but `TerminalState` now has three — `Cancelled` is a live wire tag assigned at
`reducer/apply/record.rs:278`. The test passes for an unrelated reason and no longer pins
what its name claims.

### F25 — Internal PR bookkeeping leaks into a user-facing runtime error

`crates/finstack-ai-kernel/src/events/envelope.rs:251`

`EventError::CorrelationMismatch { reason: "PR-008 durable event source must be run-scoped" }`
— a stable error string in a 1.0.0 crate referring to an internal logical-PR number.
Reachable: `RecordEnvelope::try_new` only enforces `run_id` presence for `RunAccepted`
bodies (`records/validate.rs:189-212`).

## Low severity

Cleanliness only; no behavioural impact.

| ID | Finding | Location |
| --- | --- | --- |
| F26 | Sixteen logical-PR bookkeeping references remain in public rustdoc (`RECORD_KIND_VERSION`, `RunEventBody`, `ReducerStageOutcome`, `RunLimits`, …). docs.rs publication is disabled, so these are internal-reader noise rather than shipped docs. | `records/mod.rs:18` + 15 sites |
| F27 | `allows_child_attenuation` doc is wrong in two ways — child-only extension keys are accepted unconditionally, and the subset direction is inverted relative to the text. | `policy/limits.rs:431` |
| F28 | Counter overflow for turns, model requests, tool calls, retries and context bytes returns `invalid_input_payload` rather than a terminal limit failure. Present on the apply side too (`apply/record.rs:78-89, 105-112`, `apply/effects.rs:54-58`). | `reducer/decide/limit.rs:104-160` |
| F29 | `serialize_micros` / `deserialize_micros` duplicated token-for-token across two modules. | `policy/limits.rs:610-633`, `refs/error.rs:85-108` |
| F30 | Tool-result decoding reimplemented in apply instead of reusing `decode_tool_result`. | `reducer/apply/tools.rs:181` |
| F31 | Dead `issues.is_empty()` guard — unreachable because `input.validate()` already rejects it upstream. | `reducer/decide/output.rs:240` |
| F32 | Unreachable `max_retries` re-check duplicating limit-dimension enforcement. | `reducer/decide/stage.rs:349` |
| F33 | Inert `#[serde(default)]` attributes on `ErrorDescriptor`'s Serialize-only derive. | `primitives/error.rs:292, 295` |
| F34 | `OpaqueBlock::try_new` doc describes a blob-reference payload variant that does not exist — `OpaquePayload` has exactly `Bytes` and `Json`. | `content/opaque.rs:198` |
| F35 | `Message::try_new` doc says empty content is allowed; `Tool`-role messages reject it. | `conversation/message.rs:363` |
| F36 | `drop_snapshot_records_leaves_the_tree` constructs a `SnapshotWritten`, binds it to `_`, and asserts nothing about dropping snapshot records. | `conversation/tests.rs:368` |
| F37 | Module docs in `state/` still claim "schema-1" / "v1 ceilings" for code spanning six schema versions. | `state/mod.rs:194` + 4 sites |

## Suggested order of work

1. **Cheap, and they prevent recurrence.** F3 and F4 are one-line changes each (`.max(N)`,
   `>= 4`) with obvious regression tests. F10 is the structural fix that would have caught
   both — add byte-exact known-answer fixtures for the state hash at every version.
2. **They break durability.** F1 and F2 share a root cause and a single fix: stop trusting
   `is_human_readable()` anywhere reachable through serde's internally-tagged
   `ContentDeserializer`. A CBOR round-trip fixture over every `ContentBlock` variant with
   non-empty payloads closes the class.
3. **Then:** F5 (snapshot fidelity), F7 and F8 (both narrow, both with clear fixes), F6 (one
   comparison), and F11 (mechanical — the projection types already exist).
4. F9 needs maintainer judgment before any change, for the reason given in the finding.

## Method and confidence

Ten reviewers each read an assigned module cluster in full against its governing TDD
sections. Every finding was then handed to an independent adversarial verifier instructed to
refute by default and to re-read the cited spec text rather than trust the reviewer's quote.

**55 findings were raised; 18 were refuted** — including one whose citations were entirely
fabricated, and several where the code observation was accurate but the claimed spec
violation was not. Only survivors appear above, with duplicate reports of the same defect
merged into a single entry.

F1, F11 and F12 were confirmed directly against source or by executing a probe, not by
trusting an agent report. Where two agents disagreed, F9 records the disagreement rather
than picking a side.

## Phase 13 dispositions (HEAD, PR-080)

These rows record status at the Phase 13 authorization baseline. They do not rewrite the
finding bodies above and do not claim merge, gate, or completion.

| ID | Disposition | Notes |
| --- | --- | --- |
| F1 | Fixed at HEAD | Do not re-implement. |
| F2 | Fixed at HEAD | Do not re-implement. |
| F3 | Fixed at HEAD | Do not re-implement. |
| F9 | Resolved by this program's #1 and #10 | Ordering fact confirmed; #10 supplies the concrete redelivery re-count. Not a fresh contested item. See [kernel-remediation-review.md](kernel-remediation-review.md). |
| F13 | Closed as finding #5 Option A | TDD 0.20: no fabricated observed charge; both reserve twins deleted. |
| F22 | Fixed at HEAD | `derived_event_kind` is crate-internal and its ordinal/version contract has a direct table-driven test. |
| F23 | Fixed at HEAD | Live `state_hash` rustdoc already covers versions 1–6. Remaining #23 rustdoc (not this entry) stays with PR-081. |
