# ADR-041: Mid-run capability activation and variants

## Status

Accepted

## Date

2026-08-17

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

FR-06 adds model-facing mid-run activation (`capability_list` /
`capability_activate`). Three existing mechanisms collide with that path
and must be decided together:

1. **C-1.** `ModelCapabilityVariant` in
   `crates/finstack-ai/src/agent/handle.rs` is a fully separate resolved
   `Agent` per model-activated capability. `resolve_model_variants`
   builds those variants. `AgentRunRequest.capability` selects one at
   `Agent::start`. `structured_output` is cloned onto every variant in
   `try_with_output_schema` (`handle.rs` around the clone at the
   structured-output assignment). This is a competing run-start design,
   not a partial mid-run path.
2. **C-3.** `ResolvedMiddlewareChain::try_new` counts
   `MiddlewareRole::ContextCompactor` and rejects more than one. Its
   contract resolves the chain before any run starts.
   `ResolvedAgentLock.middleware_chain_digest` and
   `PipelinePosition.chain_digest` are stamped from that lock-time
   digest. Two different digests in one run are undefined for replay.
3. **C-4.** Kernel `decide_capabilities_activated` rejects
   `CapabilitiesActivated` unless `state.phase == Some(RunPhase::BeforeRun)`.
   Mid-run submit is `InvalidPhaseInput` today. The apply-time batch
   shape matrix also accepts that record only at `BeforeRun`. Identical
   complete-set re-activation is a no-op; a duplicate id in one
   activation is `invalid_input_payload`.

The reducer applies `state.active_capabilities = activation.active.clone()`.
A second activation that names only the newly selected id **drops**
already-active capabilities (C-2). Activate must therefore submit the
complete intended set.

Anthropic and OpenAI prompt-cache breakpoints key on the tool list.
Mid-run activation invalidates that cached prefix. This record does not
solve that. It interacts with later FR-04 and FR-07 work.

Python `Capability` is documented as instructions-only
(`bindings/finstack-ai-python/python/finstack_ai/_finstack_ai.pyi`).
That binding answer must be stated, not left implicit.

No seventh port. No new `RecordBody` variant. Unknown journal kinds stay
fatal. FR-10 load-time SKILL.md / plugin import is out of scope.

## Decision

### 10a / C-1 — variants and mid-run activation coexist

The two mechanisms **coexist**.

- `AgentRunRequest.capability` still selects the run-start
  `ModelCapabilityVariant`. `None` runs the agent that was called. An
  unknown catalog id fails closed.
- `capability_catalog()` and `compact_capability_catalog()` remain public
  and are not removed. `ModelCapabilityVariant` remains `pub(super)`.
- Mid-run `capability_activate` unions onto the *current* resolved
  agent's capability set — the variant already chosen at start. It does
  not re-resolve the agent, does not switch variants, and does not
  rebuild `Arc<ResolvedToolCatalog>`.
- `structured_output` remains cloned onto every variant at handle
  construction. Mid-run activation does not change structured-output
  ownership.

Rejected: superseding variants and removing `capability_catalog()` or
`AgentRunRequest.capability`. Those are public surfaces. Removals would
be major.

### 10b / C-3 — lock-time union, dispatch-time mask (option b)

Select option **(b)**:

- Resolve all capability-contributed middleware, toolsets, and context
  providers into the lock-time union, including inactive `Model`
  capabilities. `ResolvedMiddlewareChain::try_new` runs over that full
  union. A second `ContextCompactor` in an inactive capability fails
  **resolution**, not activation (`MIDDLEWARE_RESOLUTION_INVALID`).
- Gate at dispatch with an active mask derived from kernel
  `active_capabilities`. Contributed toolsets, context providers, and
  middleware become live only after `CapabilitiesActivated` commits.
- `middleware_chain_digest` and `PipelinePosition.chain_digest` stay
  constant for the run. No run emits two different `chain_digest`
  values without an explicit journaled epoch boundary.
- Option (b) does not change effect payloads or durable event order.
  **No RFC is required.**

Rejected:

- **(a)** Capabilities may not contribute middleware. Rejected: it
  forbids a declared CapabilitySpec field and still leaves the
  compaction-union question unanswered for toolsets and providers.
- **(c)** Per-activation-epoch digest. Rejected: it changes what is
  stamped on `PipelinePosition` and would require an RFC plus a
  journaled epoch boundary. Unknown journal kinds are fatal on 1.0.x.

### 10c / C-4 — kernel phase gate

Today the kernel rejects mid-run `CapabilitiesActivated` with
`InvalidPhaseInput` (and apply-time `InvalidRecordOrder` if the batch
shape is not `BeforeRun`).

**Allowed phases** for `CapabilitiesActivated`:

- `RunPhase::BeforeRun` — existing initial activation after `AcceptRun`.
- `RunPhase::AfterToolBatch` — the named mid-run phase, after the
  activating tool batch has committed and before `AfterToolBatch`
  continues.

Other phases remain `InvalidPhaseInput`. The change is additive: the
`BeforeRun` check is not deleted.

Unchanged:

- Identical complete-set / digest re-activation remains a no-op
  (`duplicate_decision`).
- Duplicate capability id in one activation remains
  `invalid_input_payload`.
- `prior_plan_digest` must still equal `state.resolved_plan_digest`.
- Reuse `RecordBody::CapabilitiesActivated`. Do not add a record
  variant.

Recovery reconstructs the dispatch mask from `ResolvedAgentLock` (full
union, including inactive capabilities) plus the journaled
`active_capabilities` chain, or fails closed. Silent divergence is not
allowed. `prior_plan_digest` chains across successive mid-run
activations.

### C-2 — additions-only activate

`capability_activate` is additions-only. The runtime computes
`current ∪ named` and submits that complete sorted set. A call naming
one id must not drop already-active capabilities. The model may not
shrink the set. Concurrent activations are bounded; overflow fails the
tool call with a stable error and does not evict.

### Binding parity

**Python stays instruction-only.** The Python `Capability` constructor
continues to accept `id`, `description`, `instructions`, and
`activation` only. It does not gain toolset, context-provider, or
middleware references. WASM matches that instruction-only surface.

Native Rust `CapabilitySpec` may still contribute registered Toolset,
ContextProvider, and Middleware references. Untrusted capability
contributions (including every `Model` capability) cannot set
`ContextProviderDescriptor.trusted_application_instructions`. Capability
instructions are never a path to that flag.

`ResolvedAgentLock` digest covers the full capability set, including
unactivated definitions.

### Prompt-cache invalidation

Mid-run activation changes the tool list visible to the next model
request and therefore invalidates Anthropic/OpenAI prompt-cache
prefixes. This ADR records that cost. It does not change cache
breakpoints, vendor adapters, or retry policy.

### FR-10

Load-time SKILL.md / plugin import remains a stub. This ADR does not
authorize an importer.

## Consequences

- Run-start variant selection and mid-run mask union are both legal on
  the same run. Mid-run activation never replaces the chosen variant
  agent.
- Compaction ownership is a resolution-time property of the full
  capability union. Hosts cannot hide a second late-tier compacting
  owner behind an inactive capability.
- Replay sees one `chain_digest` for the run. Recovery reapplies the
  journaled mask over the lock-time union.
- Python and WASM adopters keep the instruction-only `Capability` API.
  Native hosts that attach executable refs to a capability must register
  those handles without placing them on the base spec.
- Prompt-cache miss after activation is expected. Do not treat it as a
  defect of this change.

## Rejected alternatives

**Supersede `ModelCapabilityVariant` and remove run-start selection.**
Rejected: public `capability_catalog()` and `AgentRunRequest.capability`
would become a major break; the two mechanisms answer different
questions (which variant starts vs which lock-time members unmask).

**Re-resolve the agent mid-run.** Rejected: it would change
`chain_digest`, rebuild catalogs, and break commit-before-effect replay.

**Change `PipelinePosition.chain_digest` per activation (option c).**
Rejected: effect-payload change requires an RFC. Option (b) avoids that.

**Delete the `BeforeRun` phase check and accept `CapabilitiesActivated`
in every non-terminal phase.** Rejected: the allowed set must be named.
`AfterToolBatch` is the mid-run commit window after the activate tool
settles.

**Add a new `RecordBody` variant for mid-run activation.** Rejected:
reuse `CapabilitiesActivated`. A new kind is fatal to 1.0.x journals.

**Let `capability_activate` submit only the named id (set replacement).**
Rejected: C-2. The reducer replaces the complete set.

**Give Python toolset/context/middleware refs in this change.**
Rejected: Python stays instruction-only. Stating that is the binding
decision.

**Solve prompt-cache invalidation here.** Rejected: vendor cache keys
are FR-04 / FR-07 adjacent.

## Compatibility and schema-change classification

Additive kernel phase-matrix widening for an existing record kind.
`CapabilitiesActivated` payload, `prior_plan_digest` matching, identical
re-activation no-op, and duplicate-id reject are unchanged. Lock
`components` may now include inactive capability contributions; lock
`capabilities` already listed those definitions. No new journal kind.
No effect-payload change. No RFC.

Public Rust surfaces `capability_catalog()` and
`AgentRunRequest.capability` are preserved. Python and WASM Capability
APIs stay instruction-only.

## Security classification

- References: SEC-INV-001; TM-01
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: Untrusted (`Model` and later imported)
  capabilities cannot set `trusted_application_instructions`. Recovery
  fails closed when the journaled active set is not a subset of the
  lock-time union. In-process skill/toolset leaves remain T1 host
  authority, not an isolation boundary. No new kernel port.

## Affected requirements, design, and delivery

- Affected requirements: FR-CAP, FR-06
- Affected Technical Design: Technical Design §10 (declarative
  capabilities) and the `CapabilitiesActivated` phase note in §22.7
- Architecture Specification §25 (ADR-008 / ADR-020 capability
  decisions remain; this record adds mid-run mask semantics)
- Implementation: FR-06 Tasks 10d–10j
- FR-10 importer is not authorized

Planning files under `docs/planning/` are not edited by this record.
The §22.7 sentence that `CapabilitiesActivated` is accepted only at
`BeforeRun` is superseded in implementation meaning by this ADR's
additive `AfterToolBatch` window.

## Supersession metadata

None. This ADR supersedes no prior ADR and is not superseded.
It refines ADR-008 and ADR-020 without replacing them.

## Reconsideration conditions

May change only through a new superseding ADR. Reopening option (c)
(per-activation digest), removing run-start variants, adding a
`RecordBody` variant, or changing the Python instruction-only surface
requires that path. A later FR-10 importer requires its own ADR.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute FR-06 Tasks
  10d–10j against the decisions above, locally only, without
  publication
- Implementation evidence: Partial — local activation `bc0a12fc0b5e836f810c1d2bf1847f5c7b331c7a` and honest
  prompt-cache bust `8fd9f9f2ecc256c40583e987209a1f444b1a7915`
  (`cargo test -p finstack-ai-provider-gateway --locked`;
  `cargo test -p finstack-ai-provider-anthropic --locked --lib request::`;
  `cargo test -p finstack-ai-provider-openai --locked --lib request::`).
  No published evidence id. Prefix reuse after activation remains invalid.
