# Proposal 0001: Ecosystem and capability gaps after 1.0

- Status: **Draft feature request**
- Author: me@jeickmeier.com
- Date: 2026-08-17
- Baseline: workspace `1.0.0` (local tag `v1.0.0`, registries unpublished)

## What this document is and is not

This is a **feature request**, not a register and not a gate claim. It does
not redefine product scope, acceptance outcomes, or compatibility policy.
[`docs/planning/`](../planning/README.md) remains authoritative for
requirements and sequencing; [`docs/implementation/`](../implementation/README.md)
remains the live control surface.

Every item below that touches contract meaning requires an ADR, and every
item that touches journal schemas, event order, WIT worlds, or the remote /
process protocols additionally requires a public RFC per
[`docs/rfcs/README.md`](../rfcs/README.md). No item here is authorized work.

Items are classified against the invariants in
[`docs/site/concept.md`](../site/concept.md):

- **G-01** — a default kernel / runtime / SDK graph depends on no workflow,
  Temporal, Restate, Wasmtime, or remote server crate.
- **Six ports, no seventh.** Model, Toolset, ContextProvider, Middleware,
  JournalStore, Observer.
- **Commit-before-effect.** Durable intent commits before external dispatch.
- **Deterministic, synchronous, I/O-free kernel.**
- In-process native / Python / JS extensions inherit host authority and are
  never described as isolated.

## Motivation

External comparison against [`deepseek-ai/deepseek-harness`](https://github.com/deepseek-ai/deepseek-harness)
(TypeScript agent harness, `0.1.0-rc.7`, developer preview) surfaced two
distinct classes of gap:

1. **Internal debt.** One of the six ports has no production driver, and the
   consequence has already propagated into a non-functional compaction leg.
   This is documented candidly in-tree but it means the six-port architecture
   is currently a five-port architecture.
2. **Ecosystem isolation.** The repository has no MCP surface, one hand-written
   provider per vendor, and no model-facing delegation or capability-activation
   toolset. Adopters cannot reach the tool and provider ecosystem that has
   formed around agent runtimes.

The first class is a correctness obligation against work already claimed. The
second is a strategic reach question. They are sequenced accordingly: **Tier 1
before Tier 2.** Shipping ecosystem surface on top of an unwired port would
multiply the debt.

## Summary table

| ID | Feature | Tier | New port? | Compatibility class | Needs RFC |
| --- | --- | --- | --- | --- | --- |
| FR-01 | `ContextProvider` production driver | 1 | No | Additive runtime behavior | No |
| FR-02 | Compaction `summarize` design slot | 1 | No | Contract meaning — ADR | Likely |
| FR-03 | MCP client Toolset + ContextProvider adapter | 2 | No | Additive leaf crate | No |
| FR-04 | Config-driven multi-provider model adapter | 2 | No | Additive leaf crate | No |
| FR-05 | Subagent toolset over `AgentInvoker` | 3 | No (host feature) | Additive leaf crate | No |
| FR-06 | Skills over `CapabilitySpec` | 3 | No | Additive leaf + spec fields | No |
| FR-07 | Provider resilience policy (retry / backoff) | 3 | No | Additive leaf crate | No |
| FR-08 | OS-level process confinement for shell / fs | 4 | No (internal service) | Additive, fail-closed | No |
| FR-09 | Registry publication of `1.0.0` | 4 | n/a | Release track | No |

---

# Tier 1 — Correctness debt

## FR-01: Wire the `ContextProvider` production driver

**Priority: highest. Everything in Tier 2 is worth less until this lands.**

### Problem

The `ContextProvider` port is fully specified, has a published conformance
suite (`check_context_conformance`), has two shipped implementations
(`finstack-ai-context-memory`, `finstack-ai-context-repository`), and can be
registered and resolved through the SDK — but nothing calls `collect()` during
a live run.

Evidence in-tree, stated by the runtime itself:

> the `ContextProvider` port has **no production driver** — no [live run]
> collects a contribution or assembles context
>
> — [crates/finstack-ai-runtime/src/exec/stage_settlement/mod.rs:49](../../crates/finstack-ai-runtime/src/exec/stage_settlement/mod.rs)

The `StageInput::PrepareContext` variant exists in the middleware stage enum,
and `ContextCallContext` carries `provider_index` and `chain_digest` for a
locked provider chain, so the contract anticipates the driver. The driver is
simply absent.

### Consequence beyond the port itself

This is not an isolated hole. `validate_compaction_result` requires the last
source entry handed to a compactor to be `protected`, and `protected` is
authoritative-from-the-context-port precisely so a compactor cannot certify
itself. With no context driver, no protected item is ever produced, every
source entry is structurally unprotected, and **every compaction result is
rejected** with `compaction_result_invalid`. Two shipped middleware crates and
the `coding` example composition are affected.

### Proposal

Implement the `prepare_context` production driver in the runtime execution
path, between run acceptance and `before_model`:

1. Resolve the locked provider chain in `AgentSpec` order; freeze
   `chain_digest` at agent resolution, not per run.
2. For each provider in order, commit the context-request record, then call
   `collect()` under the shared `RunCallContext` deadline, budget, and
   cancellation — commit-before-effect, same as Model and Toolset.
3. Apply per-provider `ContextBudget` ceilings (`max_tokens`, `max_bytes`,
   `max_items`) and a chain-level aggregate ceiling. Overrun truncates
   deterministically at the item boundary and records the truncation; it does
   not fail the run.
4. Assemble contributions into the committed context projection, setting
   `protected` from provider authority — specifically from
   `ContextProviderDescriptor::trusted_application_instructions` plus the
   chain position, never from anything the compactor can influence.
5. Drive `StageInput::PrepareContext` middleware around the assembled result.
6. Call `reconcile()` for outstanding context effects on recovery, using the
   existing `ContextReconcileResult` variants.

### Ordering constraint

Contributions are **data, never authority** — this is already stated on the
port and must survive the driver. A context provider must not be able to
promote itself to a trusted-instruction source at collect time; the trust bit
is descriptor-locked at resolution.

### Acceptance criteria

- A live run with a registered context provider produces a committed context
  projection derivable from the journal alone.
- Sliding-window compaction completes end-to-end in the `coding` example.
- `check_context_conformance` passes against a driver-backed run, not only
  against a direct port call.
- Golden traces cover: empty chain, single provider, ordered multi-provider,
  budget truncation, provider error, and crash-then-reconcile at each of the
  five `ContextReconcileResult` variants.
- [`docs/site/middleware.md`](../site/middleware.md) "why compaction cannot
  complete" section is deleted, not amended.

### Compatibility

Additive runtime behavior against a port already published at suite version
1.0.0. No journal schema change if context records already have a reserved
`RecordBody` variant; confirm before implementation. If a new record kind is
required, this becomes RFC-bearing.

---

## FR-02: A design slot for `RequestCompactionModel`

### Problem

The `summarize` compaction strategy returns `RequestCompactionModel`, which
per the runtime's own module contract *"has no design slot at all"*. It needs
a committed child model effect attached to a committed middleware parent, plus
re-entry into the chain carrying the model's answer. Under the aggregate-fold
design a stage settles exactly once and there is no committed middleware
parent, so neither half has anywhere to live.

Unlike FR-01, **this one is not unblocked by wiring the context port.** The two
causes are independent and the in-tree analysis is explicit that fixing either
alone is insufficient.

### Proposal — three candidate shapes, decision deferred to ADR

**(a) Middleware-owned child model effect.** Give `before_model` middleware
invocations a committed parent envelope so a child model effect can attach,
and allow a single bounded chain re-entry carrying the result. Most direct;
weakens "a stage settles exactly once" and makes middleware effect-bearing,
which contradicts the current statement that a middleware invocation is never
a committed effect (see the deprecation note on `Middleware::reconcile`).

**(b) Compaction as a runtime-driven phase, not middleware.** Promote
model-assisted compaction out of the middleware chain into an explicit runtime
phase between `prepare_context` and `before_model`, with its own committed
effect identity. Preserves middleware purity; adds a runtime phase and a
second model-request path that must share the `validate_model_request`
contract.

**(c) Two-pass settlement.** Let a stage return a *deferred* outcome that
settles on a second pass after the runtime fulfills a declared model request.
Keeps compaction in middleware and keeps effects in the runtime; costs a
settlement-model change that touches recovery semantics for every stage.

Recommendation: **(b)**. It is the only option that leaves `Middleware`
pure with respect to external state, which is the property the current
recovery design depends on (recovery re-runs the whole chain). It also gives
model-assisted compaction a natural home for its own token accounting against
the locked context profile.

### Non-goal

Do not fabricate the `protected` bit at source assembly to make deterministic
strategies pass. The in-tree analysis names this explicitly as the wrong fix:
it would let the constrained party certify itself. FR-01 is the correct
unblock for that half.

### Acceptance criteria

- ADR recorded selecting a shape, with the rejected alternatives named.
- `summarize` completes against a scripted model with deterministic output.
- Child model effect appears in the journal with parent linkage and is
  replayable.
- Crash between the compaction model request and chain re-entry recovers
  at-least-once without duplicating the summary in the committed context.
- Token accounting for the compaction request is charged against the same
  budget as the primary request; no unmetered model call exists.

### Compatibility

Contract meaning. **ADR required.** Public RFC required if the selected shape
changes runtime event order or adds a `RecordBody` variant — likely under
(a) and (b).

---

# Tier 2 — Ecosystem interop

## FR-03: MCP client as a Toolset and ContextProvider adapter

### Problem

`grep -ri mcp` over `crates/`, `extensions/`, `plugins/`, `bindings/`, and
`docs/site/` returns **zero matches**. MCP is the de facto distribution
channel for agent tools. An adopter with an existing MCP server cannot use it
here without writing a bespoke `Toolset`.

### Does this require a seventh port?

No — but only if scope is drawn deliberately. MCP is four capabilities plus
two notification classes, and they do not all map:

| MCP capability | Maps to | Verdict |
| --- | --- | --- |
| `tools/*` | `Toolset` port | Direct fit |
| `resources/*` | `ContextProvider` port | Direct fit, **depends on FR-01** |
| `prompts/*` | `InstructionSpec` in `CapabilitySpec` | Fit, static snapshot |
| `elicitation/*` | `InteractionRequest` / `InteractionResolution` | Strong fit — existing typed durable HITL machinery |
| `sampling/*` | — | **Out of scope. Do not implement.** |
| `notifications/tools/list_changed` | — | **Out of scope.** See below. |

**Sampling is the genuine incompatibility.** It inverts control: the server
asks the client to run a model. That requires the Model port to be re-entrant
from inside a committed tool effect, which would create a model request with
no locked context profile, no budget parent, and an effect graph the kernel
cannot linearize. Declining sampling is the correct answer, and it should be
declined loudly in the crate docs rather than silently.

**Dynamic tool lists collide with locked resolution.** `Toolset::tools()`
returns `Arc<[ToolSpec]>` — a snapshot. Composition resolves once at build
into direct handles and the per-run path performs no registry lookup, by
design. Proposal: **freeze the MCP tool catalog at agent resolution.** Handshake
and `tools/list` happen during component construction; a mid-run
`list_changed` notification is recorded as an observer event and otherwise
ignored. Adopters who need the new tools re-resolve the agent. This preserves
the lock and the `ResolvedAgentLock` digest.

### Proposal

New leaf crate `extensions/toolsets/finstack-ai-tools-mcp`, opt-in, absent
from the default SDK graph (G-01).

- Transports: stdio subprocess and streamable HTTP. Reject SSE-only servers.
- Construction-time handshake; `tools/list` and `resources/list` snapshot
  into `ToolSpec` and `ContextProviderDescriptor`.
- Each MCP tool maps to a `ToolSpec` with `side_effect`, `retry_safety`, and
  `approval` **defaulting to the most conservative class** — side-effecting,
  not retry-safe, approval-required — because MCP carries no such
  declarations. Adopters may relax per tool name in config; the default must
  never be permissive.
- `max_result_bytes` enforced client-side; oversize results truncate with a
  recorded marker rather than propagating unbounded server output.
- `reconcile()` returns `NonRepeatable` for any tool not explicitly declared
  retry-safe in config. An MCP server is an opaque external effect.
- Server identity, command line or URL, and the resolved tool-name set are
  captured in the component invocation digest so the lock detects drift.

### Trust

An MCP server is external code reached over a transport the host controls.
Classify stdio servers as **T1** — a spawned subprocess inherits host
authority and this crate is not a sandbox. Classify HTTP servers as **T4**,
consistent with remote clients. Document both; do not describe either as
isolated. FR-08 confinement applies to the stdio case and should be noted as
a follow-on, not a precondition.

### Acceptance criteria

- `check_toolset_conformance` passes against a scripted MCP server fixture.
- No network or subprocess activity on `import` / construction of the default
  SDK graph.
- Approval floor holds: a side-effecting MCP tool cannot run without a durable
  interaction resolution.
- Golden traces for handshake, tool call, oversize truncation, server crash
  mid-call, and transport timeout.
- Crate docs state plainly that sampling is unsupported and why.

### Compatibility

Additive leaf crate. No kernel, runtime, or port change. **Depends on FR-01**
for the resources half; the tools half can land first.

---

## FR-04: Config-driven multi-provider model adapter

### Problem

Three hand-written providers (`openai`, `anthropic`, `ollama`), each a crate.
Every new vendor, OpenAI-compatible gateway, or self-hosted endpoint is a code
change and a release. `docs/site/provider.md` states that generic Chat
Completions gateways are deliberately not a first-party provider — reasonable
at preview, expensive at scale.

deepseek-harness solves this with a generic adapter where *"an
OpenAI-compatible gateway, a self-hosted server, or a provider newer than the
installed catalog is configuration rather than a code change."* The pattern is
sound and portable.

### The constraint they do not have and we do

`validate_model_request` is strict: it requires a `LockedModelContextProfile`
whose `estimator` matches what `estimate_input_tokens` returns, and it enforces
`hard_input_bytes`, `context_window_tokens`, `reserved_output_tokens`, and
`provider_overhead_tokens`. A config-driven provider cannot skip this. So the
config schema must carry, per model, exactly what the validator needs — this
is the design, not an afterthought:

```text
route:
  wire_protocol: openai_responses | openai_chat | anthropic_messages | ollama_chat
  endpoint:      <https URL or loopback>
  auth:          <credential reference, never a literal>
  models:
    <model-name>:
      context_window_tokens:     <u64>
      reserved_output_tokens:    <u64>
      provider_overhead_tokens:  <u64>
      estimator:                 <named estimator id>
      capabilities:              <ModelCapabilities flags>
```

A route that omits any required field fails at construction, not at first
request. There is no inferred default context window — guessing one would
convert a hard validation error into silent truncation.

### Proposal

New leaf crate `extensions/providers/finstack-ai-provider-gateway`.

- Implements `Model` over a small set of named wire protocols. Protocol
  selection is config, not a trait.
- Reuses the existing per-vendor stream normalization from the three current
  crates; extract shared normalization into `provider_util` rather than
  duplicating it.
- Credential references resolve per request through the redacted
  `Authentication` wrapper. Never reads environment variables directly, matching
  the existing factory rule. A configured reference that resolves to nothing
  fails the request explicitly rather than falling through to ambient
  credentials.
- Non-loopback endpoints require HTTPS. No plaintext bearer, matching the
  server crate's existing posture.
- The three existing provider crates stay. This adapter is for reach, not
  replacement, and the vendor crates remain the reference implementations and
  golden-trace sources.

### Acceptance criteria

- `check_model_conformance` passes for each supported wire protocol.
- A route pointing at a scripted HTTP fixture round-trips without a code
  change to the crate.
- Estimator mismatch, missing context profile, and plaintext non-loopback
  endpoints all fail at construction or validation with the existing stable
  error codes — no new error taxonomy.
- No secret reaches `AgentSpec`, bundle defaults, resolution locks, or logs.
  Existing redaction tests extended to cover the config path.

### Compatibility

Additive leaf crate. Default SDK graph unchanged; no client constructed on
import.

---

# Tier 3 — Agent capability surface

## FR-05: Subagent toolset over the existing `AgentInvoker`

### Problem

The kernel and runtime already carry most of a subagent system and none of it
is reachable by a model:

- `ChildPlacement` — `CompatibleLaneInParentSession`, `IsolatedChildSession`,
  `RemoteChildSession` with a frozen `RemoteRouteRef`.
- `ChildRunLocator` with placement validation.
- `AgentInvoker` trait plus `ChildRunRequest` / `ChildRunHandle` in
  [crates/finstack-ai-runtime/src/services/agent_invoker.rs](../../crates/finstack-ai-runtime/src/services/agent_invoker.rs).
- `HostFeature::AgentInvoker` in the bundle catalog and `RequiredServices`.
- `ChildRunPolicy { Deny | Allow { max_depth } }` in `AgentSpec`, defaulting
  to `Deny`.
- Cancellation fanout across all three placements in `services/session.rs`.
- Full propagation policy for cancellation, deadline, budget, and principal.

**`AgentInvoker` is a host feature, not a port.** Building on it costs no
seventh port — this is the key architectural finding, and it means the
remaining work is a leaf crate, not a core change.

What is missing is the model-facing surface: a toolset that lets a model
request delegation, and a report path for the child's result.

### Proposal

New leaf crate `extensions/toolsets/finstack-ai-tools-subagent`.

- Tools: `subagent_start`, `subagent_await`, `subagent_cancel`. Keep the
  surface under the NFR-DX-002 100-line policy target.
- Every call routes through the host `AgentInvoker`; the toolset holds no
  invocation authority of its own.
- `ChildRunPolicy` is enforced by the runtime, not the toolset. A model
  request that exceeds `max_depth` fails with the existing error, and the
  toolset does not get to interpret the policy.
- Child agent selection is restricted to an allow-list frozen in the toolset's
  component invocation. A model may not name an arbitrary agent.
- Budget: child runs draw on the parent reservation under the existing
  `BudgetPropagation::SharedScope`. A subagent cannot mint budget.
- `reconcile()` maps to the child run's committed state — a child run is
  already journaled, so unlike an MCP call this is genuinely recoverable
  rather than `Unknown`.

### Deliberately out of scope

Delegation to *external* agent products (Claude Code, Codex, ACP peers) as
deepseek-harness does. That is a protocol-compatibility project, not a
capability gap, and `RemoteChildSession` plus the remote protocol is the seam
where it would eventually attach. Not proposed here.

### Acceptance criteria

- A parent run delegates, awaits, and incorporates a child result, fully
  replayable from the journal.
- `ChildRunPolicy::Deny` (the default) rejects delegation with no partial
  child state committed.
- Parent cancellation cascades to children across all three placements — the
  fanout path already exists and gains model-facing test coverage.
- Depth limit, budget exhaustion, and child failure each surface as a normal
  tool result, not a run abort.

### Compatibility

Additive leaf crate plus a `HostFeature` requirement. No port change.

---

## FR-06: Skills over `CapabilitySpec`

### Problem

There is no progressive-disclosure mechanism: everything a resolved agent can
do is in its prompt and tool catalog from the first token. This is the pattern
deepseek-harness ships as `packages/skill/*` and it is now a baseline
expectation.

### The substrate already exists

`CapabilitySpec` is very nearly this already:

```rust
pub struct CapabilitySpec {
    pub id: CapabilityId,
    pub description: Arc<str>,
    pub instructions: Arc<[InstructionSpec]>,
    pub toolsets: Arc<[ComponentRef]>,
    pub context_providers: Arc<[ComponentRef]>,
    pub middleware: Arc<[ComponentRef]>,
    pub activation: CapabilityActivation,
}
```

A capability bundles instructions, toolsets, context providers, and middleware
behind an activation mode, and the kernel already carries `ActiveCapability`
and `CapabilityActivationSource`. `AgentRunRequest.capability` already exists
as an additive field. **A skill is a capability with a model-triggered
activation source** — the concept does not need inventing, only completing.

### Proposal

1. Add a model-triggered `CapabilityActivationSource` variant alongside the
   existing sources, so a model can request activation rather than only the
   host selecting one at run acceptance.
2. Small leaf toolset exposing `capability_list` and `capability_activate`
   over the resolved capability set. Descriptions only until activation —
   that is the disclosure boundary.
3. Activation commits a record before any contributed component is consulted.
   Contributed toolsets, context providers, and middleware become live for
   subsequent steps in the same run, and the activation is replayable.
4. Bound the active set: a configured maximum concurrent capability count, and
   a hard rule that activation may not introduce a second late-tier
   `before_model` compaction owner — the existing single-owner invariant must
   hold across activation, and `MiddlewareDescriptor` validation is the place
   to enforce it.
5. Filesystem-backed capability discovery is **not** proposed. Capabilities
   stay declared in `AgentSpec` and frozen in the resolution lock. Scanning a
   directory at runtime would break the lock and reintroduce registry lookup
   on the per-run path.

### Acceptance criteria

- A run activates a capability mid-run and the newly contributed tools appear
  in the next model request, derivable from the journal alone.
- Activation of a capability carrying a second compaction owner is rejected at
  resolution, not at activation time.
- Instructions contributed by an untrusted capability cannot set
  `trusted_application_instructions`.
- `ResolvedAgentLock` digest covers the full capability set including
  unactivated ones.

### Compatibility

Additive `CapabilityActivationSource` variant. If that enum is public and not
`#[non_exhaustive]`, this is a source break — **check before implementing**, and
note the precedent already recorded for `AgentRunRequest.capability` in the
1.0 compatibility matrix. Possible RFC if activation adds a runtime event.

---

## FR-07: Provider resilience policy

### Problem

No shared retry, backoff, or rate-limit handling across providers. Each
provider crate handles transport failure independently, and an adopter facing
a 429 gets a run failure. deepseek-harness isolates this as `llm-retry`.

### Why this is not trivial here

Retry interacts directly with commit-before-effect. A model request is a
committed effect; retrying it is either same-identity retry (safe, the effect
id is the idempotency key) or a new effect (a second committed request). The
distinction must be explicit, and the current `ModelError` taxonomy already
separates validation, limit, and provider classes — retry policy keys off that
classification rather than off HTTP status codes.

### Proposal

A shared, provider-agnostic policy applied by the runtime model driver, not by
each provider crate:

- Retry only errors classified retry-safe by the existing `ModelError` class.
  Never retry validation or limit errors.
- Same-identity retry preserves the committed effect id. No new record.
- Bounded attempts, exponential backoff with jitter, and a deadline check
  before each attempt — a retry that cannot complete before the run deadline is
  not attempted.
- Every attempt emits an observer event. Retries are never silent.
- Honor `Retry-After` when the provider supplies it, capped by the run
  deadline.
- Policy is configured per agent, defaulting to **no retry**, preserving
  current behavior for existing adopters.

### Acceptance criteria

- Golden traces prove the journal is identical between a first-attempt success
  and a success after N retries.
- Deadline is never exceeded by backoff.
- `check_model_conformance` unchanged — this is driver behavior, not a port
  contract change.

### Compatibility

Additive runtime behavior, off by default. No port change.

---

# Tier 4 — Operational hardening

## FR-08: OS-level process confinement for shell and filesystem toolsets

### Problem

`finstack-ai-tools-shell` is documented as "deny-by-default argv, empty env,
timeout." That is **policy, not confinement.** A permitted binary runs with
full host authority. The filesystem toolset is stronger — capability-scoped
root, symlink-escape rejection, fail-closed on platforms lacking safe
primitives — but a shell tool that can invoke an editor undoes it.

The WIT/Wasmtime plugin host (T3, deny-by-default WASI, fuel and limits,
signature policy) is real isolation, but it confines *plugin guest code*, not
*spawned subprocesses*. These are different threat surfaces and the plugin host
does not cover this one.

deepseek-harness ships Landlock (via a `native/landlock-run` helper), Windows
ACL sandboxing, and E2B remote sandboxing as distinct backends.

### Proposal

An internal `ProcessConfinement` service — **not a port** — consumed by the
shell toolset and any future subprocess consumer:

- Linux: Landlock (5.13+) filesystem restriction plus `no_new_privs`.
- macOS: Seatbelt profile. Note in the ADR that the API is deprecated by
  Apple; record the risk rather than hiding it.
- Windows: restricted token plus Job Object.
- **Fail closed** on any platform without the required primitive, matching the
  existing filesystem toolset precedent. A confinement request that cannot be
  satisfied must refuse to spawn, never spawn unconfined.
- The confinement profile derives from the same capability-scoped root the
  filesystem toolset already holds, so the two cannot disagree.

Remote sandbox backends (E2B and similar) are **not proposed.** They are a
separate trust class and belong behind the remote protocol if ever pursued.

### Acceptance criteria

- A confined shell tool cannot read outside its declared root, proven by a
  hostile test per platform.
- Unsupported platform refuses to spawn, with a stable error.
- Trust documentation updated. Confined subprocesses are a *reduction* in
  authority, not isolation — do not upgrade the shell toolset's T1 label
  without a threat-model review.

### Compatibility

Additive, opt-in. Existing unconfined behavior stays reachable and clearly
labeled. **Threat-model review trigger** — likely ADR.

---

## FR-09: Publish `1.0.0` to registries

### Problem

Not a code feature, but the highest-leverage item in this document.

The workspace is `1.0.0` with frozen contracts, a compatibility policy,
per-port conformance suites, and a versioned badge process. None of it is
installable: crates.io, PyPI, and npm have nothing, and the last pushed GitHub
tag is `v0.1.0`. Contract stability is only worth something to people who can
depend on the contract. Meanwhile the comparison point reached 149K stars in
four days on a repo that openly promises breaking changes — ecosystem gravity
is accruing to whoever is reachable.

The conformance badge process in particular is dead on the vine: it invites
third parties to claim `finstack-ai <port> conformance 1.0.0`, and no third
party can obtain the crates to run the suites against.

### Proposal

Sequence the publication track already described in
[`release-engineering.md`](../implementation/release-engineering.md):

1. Push `v1.0.0`.
2. Publish core crates lockstep: kernel, protocol, runtime, SDK.
3. Publish leaf extensions at the same version per
   [`1.0-leaf-versioning.md`](../implementation/1.0-leaf-versioning.md).
4. Publish the Python wheel and the npm WASM package.
5. Publish the conformance helpers as a consumable crate so third-party claims
   are actually possible.

Note that pushing tags, publishing registries, and announcing are **external
actions** and are not claimed by this document. This item records the
dependency, not the authorization.

### Acceptance criteria

- `cargo add finstack-ai` works from a clean machine.
- The offline starters in `README.md` run against published packages rather
  than a repository checkout.
- A third party can run a conformance suite without vendoring the workspace.

---

# Explicitly not proposed

These stay excluded, consistent with
[`public-ga-roadmap.md`](../implementation/public-ga-roadmap.md). Listing them
prevents this document from being read as scope creep:

- **Plugin marketplace** or hosted badge issuer.
- **Product-specific UIs.** No web UI, no CLI application. The comparison
  point's Web UI is the largest visible difference and it is correctly out of
  scope for a library.
- **PostgreSQL journal**, **native dylib ABI**, **exactly-once delivery**.
- **MCP sampling** (FR-03) and **remote sandbox backends** (FR-08).
- **External agent-product delegation** — ACP, Claude Code, Codex peers (FR-05).
- **Filesystem-scanned skill discovery** (FR-06).
- A **seventh port.** Nothing in this document requires one. If implementation
  reveals that something does, that is an ADR trigger and a stop, not a
  workaround.

# Sequencing

```text
FR-01  ─┬─> FR-02
        ├─> FR-03 (resources half; tools half is independent)
        └─> FR-06 (capability-contributed context providers)

FR-04  ── independent
FR-05  ── independent (substrate already present)
FR-07  ── independent
FR-08  ── independent; strengthens FR-03 stdio transport
FR-09  ── independent; gates external adoption of everything above
```

Recommended order: **FR-01, FR-09, FR-03 (tools), FR-04, FR-05, FR-02, FR-06,
FR-07, FR-08.**

FR-09 sits second deliberately. It is cheap relative to everything else in the
list and it is the difference between this work reaching adopters and not.

# Open questions

- Does the journal already reserve a `RecordBody` variant for context
  contributions, or does FR-01 require an RFC-bearing schema addition?
- Is `CapabilityActivationSource` `#[non_exhaustive]`? FR-06's classification
  depends on the answer.
- Should the MCP adapter (FR-03) live under `extensions/toolsets/` given that
  it also contributes context providers, or does it warrant a new
  `extensions/interop/` family?
- Does FR-02 option (b) require a new runtime event, and therefore an RFC?
- Which of these, if any, should ship as `1.1.0` versus accumulate behind a
  `1.1.0-rc` line?

# Related

- [Public GA roadmap](../implementation/public-ga-roadmap.md)
- [1.0 compatibility policy](../implementation/1.0-compatibility-policy.md)
- [Compatibility governance](../implementation/compatibility-governance.md)
- [Trust levels](../site/security-trust-levels.md)
- [Conformance suites](../site/conformance.md)
- [RFC process](../rfcs/README.md)

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[SECURITY.md](../../SECURITY.md).
