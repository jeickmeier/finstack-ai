# PR-044 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Branch: `codex/pr-044-typed-interactions`
Baseline: PR-043 review head `598f2a0c27bed021135fa16a4f50b95c206133de` (candidate `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`)
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256 `555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-044. It does **not** authorize
coding, branch creation, commit, merge, or hosted actions. The open
PR-043 envelope is not reused. `continue` does not start this PR.
Planning does not admit PR-044.

## Admission

- Phase 6 entrance is `Passed` (3/3). Do not re-record `PH6-E-entrance-*`.
- PR-039 is `Done` at `64c54e767f53faac240ab92c19a8447264e82ff8`.
- PR-040 is `Done` at `dbd10d35b223288666b2fdc0e13d03f48b5b97c3`.
- PR-041 is `Done` at `8a84293264291dbe158fae1b4e5dedcc30c74061`.
- PR-042 is `Done` at local merge `4d627711632c771d733b689e1325b2d9679ee317`.
- PR-043 is `In review` on `codex/pr-043-tool-reconciliation` (candidate
  `86f71c8fd47c0c6d721f9ae36df5c0ba90b22025`, evidence
  `598f2a0c27bed021135fa16a4f50b95c206133de`). It is the **only**
  active logical PR. PR-044 stays `Todo` until PR-043 exits active
  status (`Done`) or the user names an authorized sequential range
  that includes PR-044.
- Middleware port is present (PR-018 `Done` at
  `3fe0314c6434211e1c8f493f24401888f9609050`).
  `StageOutcome::RequestInteraction` is validated at all seven stages
  and is **not** mapped to kernel commits.
- ADR-027 is Accepted / Partial (PR-013 DTO + PR-018
  `RequestInteraction` / approval-profile evidence). This PR advances
  routing and lifecycle evidence only. Do **not** mark Implemented or
  Verified. PR-048 remains.
- ADR-025 is Accepted / Partial (model + tool runtime lifecycle;
  interaction path remains). This PR continues the interaction path
  under the original `EffectId`. Do **not** mark Verified.
- ADR-033 stays Implemented / Verified for **model** effects. Do not
  reopen it.
- ADR-013 stays Accepted / Not started in the register (Missing
  evidence). Do not silently mark Implemented.
- ADR-034 stays PR-018/PR-048.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  stays inside the existing six ports, existing PR-008 interaction
  record families, and at-least-once plus idempotency. Do not add a
  seventh port. Do not add an eighth middleware stage. Do not claim
  exactly-once external resolution.
- Adding `KernelInput` variants and a kernel-state v6 projection is
  classified pre-1.0 contract evolution (same class as v3/v4/v5). It
  does not change journal, remote-protocol, or WIT compatibility
  **policy**.
- Threat Model section 18 is triggered (interaction semantics;
  primary **TM-11**; SEC-INV-003/004; secondary TM-10/TM-14 via the
  generic effect lifecycle). Complete the review before merge. A
  threat-model review is not an ADR and does not stop coding after
  admit.
- Dependencies: PR-039–PR-043 and the middleware port. PR-043 must
  not still be the sole active PR at admit. Toolset port is present.
- Phase 6 exit bullet “generalized interaction suspension/resume
  works through Rust and Python, with approval as the first supported
  profile” is **partial evidence** from this PR. Do not infer G5.
  G5 still waits for PR-048.

## Acceptance mapping

- PR-044-A01: a process can stop after requesting any supported
  `InteractionKind` and resume after a later valid resolution
  (in-process drop + `CommitCoordinator::recover` + router resolve +
  respawn; see crash/resolution matrix).
- PR-044-A02: a denied or expired approval never dispatches the
  protected tool effect (`call()` count stays 0; no tool
  `EffectRequested` for that call).
- PR-044-A03: duplicate equivalent resolutions (`resolution_id` +
  normalized response digest) are idempotent; conflicting or late
  resolutions fail closed and are audited
  (`RecordExternalCommandRejected` when the locator/target is known;
  audit-only for unknown/unauthenticated).
- PR-044-A04: approval, choice, and review traces are
  binding-independent and use the same persisted interaction
  envelope (`InteractionRequested` / `InteractionResolved` /
  `EffectRequested { kind: Interaction }` / matching effect
  terminal). Rust and Python list/resolve go through
  `InteractionRouter`.

## Execution envelope

Authorized 2026-08-14 by `implement the plan`.

Local branch and commit are authorized. Merge to `main`, feature-branch
push, hosted PR/merge, npm publish, tag, and G5 inference are not named
and remain prohibited. The open PR-043 envelope is not reused.

`mode=stacked` (local implementation on the PR-043 review head);
`target` unset; `external actions=none`.

PR-043 remains `In review` and is not marked `Done`. This PR is the
active implementation slice; it does not close or merge PR-043.

Baseline: `598f2a0c27bed021135fa16a4f50b95c206133de`.
Branch: `codex/pr-044-typed-interactions`.

## Locked design

Do not invent a seventh port, approval-only journal records,
`InteractionRejected`, a generic `ReconcileResult`, a new
`RunPhase`, or new `RecordBody` families. PR-008 already froze
`InteractionKind`, `InteractionRequest`, `InteractionResolution`,
`InteractionExpired`, `InteractionCancelled`,
`EffectKind::Interaction`, `EffectInput::Interaction`,
`EffectOutputKind::InteractionResolution`, and
`InteractionResolutionCommand`. Activate them.

TDD §12.4 append pairing already exists in
`validate_interaction_request_pairs`:

- request batch: exactly one `InteractionRequested` ↔ one
  `EffectRequested { kind: Interaction }` with matching
  `effect_id`, `interaction_id`, and `request_digest`;
- resolve batch: `InteractionResolved` count ==
  `EffectCompleted` with `InteractionResolution` contract;
- expire batch: `InteractionExpired` count == matching
  `EffectFailed`;
- cancel batch: `InteractionCancelled` count == matching
  `EffectCancelled`.

Denial is a schema-valid `InteractionResolved`, not a new record.

### 1. Do not consume the stage cursor

`ReducerStageOutcome` must **not** gain `InteractionPrepared`.
`StageSettled` writes `stage_settlements`. If BeforeToolBatch or
BeforeFinalize consumed that cursor to request an interaction, a
later `ToolBatchPrepared` / `FinalizeAccepted` on the same cursor
would `ConflictingSettlement`.

The stage’s real work has not happened. Requesting an interaction
is a suspension, not a stage settlement.

```rust
// crates/finstack-ai-kernel/src/reducer/input.rs
KernelInput::RequestInteraction(RequestInteraction),
KernelInput::InteractionSettled(InteractionSettled),

pub struct RequestInteraction {
    pub request: InteractionRequest,
}

pub enum InteractionSettled {
    Resolved(InteractionResolution),
    Expired(InteractionExpired),
    Cancelled(InteractionCancelled),
}
```

These are command vocabulary, not journal families. Add
public-rust-api `kernel-input` fixtures. Do not add a
`ReducerStageOutcome` variant. Do not route resolutions through
`ExternalEffectCompleted` (that path cannot emit
`InteractionResolved`).

`RequestInteraction` is valid from the current expected middleware
stage phase (`BeforeRun`, `PreparingContext`, `BeforeModel`,
`AfterModel`, `BeforeToolBatch`, `AfterToolBatch`,
`BeforeFinalize`) when `pending_interaction` is `None`. It does
**not** write `stage_settlements`. It commits the atomic request
pair and enters `AwaitingInteraction`.

`InteractionSettled` is valid only in `AwaitingInteraction` with a
matching outstanding `interaction_id` / `effect_id`.

Invalid phase/input stays `invalid_phase_input`. A second
outstanding request fails closed. `RecordExternalCommandRejected`
changes no run state.

### 2. KernelState projection (v6)

Add derived outstanding-interaction state. Histories with **no**
interaction records keep their existing `state_version` and
`state_hash` (v1–v5). Applying the first interaction record
upgrades transactionally to `state_version = 6`. Same class as
TDD §22.6.4 v3.

```rust
pub struct PendingInteraction {
    pub request: InteractionRequest,
    pub prior_phase: RunPhase,
    pub cursor: StageCursor, // expected cursor at request time; still unset
}

pub struct ResolutionIdentity {
    pub interaction_id: InteractionId,
    pub settlement_digest: Digest,
}

pub struct InteractionTerminal {
    pub interaction_id: InteractionId,
    pub kind: InteractionKind,
    pub cursor: StageCursor,
    pub outcome: InteractionTerminalOutcome, // Granted | Denied | Expired | Cancelled
}

// on KernelState (v6)
pub pending_interaction: Option<PendingInteraction>,
pub resolution_identities: BTreeMap<Arc<str>, ResolutionIdentity>,
pub last_interaction_terminal: Option<InteractionTerminal>,
```

At most one outstanding interaction per run (same cardinality as
`pending_model_effect`). `list` returns 0 or 1.

`resolution_identities` is the A03 index (`resolution_id` →
normalized digest). Do not reuse `completion_identities` keys;
those index effect `completion_id`. The interaction effect still
gets a normal `EffectCompleted` / `EffectFailed` /
`EffectCancelled` identity in `completion_identities`.

`last_interaction_terminal` is how tool/middleware policy knows a
grant already released this cursor so RequireApproval does not
loop.

Snapshot encode follows existing KernelState serde. Old / unknown
snapshots still discard-and-replay (PR-041). Do **not** claim
PR-048 settlement-index rebuild. Do not add journal fields.

### 3. Apply / decide

`apply.rs` today returns `InvalidRecordOrder` for all four
interaction bodies and treats `AwaitingInteraction` as having no
valid shape. Activate:

| Phase | Valid batch |
| --- | --- |
| requesting stage phase | `EffectRequested { Interaction }` + `InteractionRequested` (append pairing already enforced) |
| `AwaitingInteraction` | `InteractionResolved` + `EffectCompleted` |
| `AwaitingInteraction` | `InteractionExpired` + `EffectFailed` |
| `AwaitingInteraction` | `InteractionCancelled` + `EffectCancelled` |
| known authorized locator | `RecordExternalCommandRejected` (no run-state change) |

On request apply: store `PendingInteraction { request, prior_phase: current phase, cursor: expected_stage_cursor }`, set phase `AwaitingInteraction`.
On resolve/expire/cancel apply: write `resolution_identities` /
`last_interaction_terminal`, clear `pending_interaction`, restore
`prior_phase`.

`apply_effect_requested` must accept `EffectKind::Interaction` in
the request batch (today only Timer / Model / Tool). Pairing with
the sibling `InteractionRequested` is mandatory.

`decide(RequestInteraction)` emits **no**
`PostCommitAction::ExecuteEffect`. There is no interaction
executor. The durable wait **is** the effect. Human/API
completion enters through `InteractionRouter`.

`decide(InteractionSettled::Resolved)` validates:

- `interaction_id` matches `pending_interaction`;
- principal/authorization pairing (existing DTO rules);
- assignee: if `assignee_hint` is present, principal matches or
  (when `delegatable`) is an attenuated delegate;
- `response` against the recorded `response_schema` /
  `response_schema_digest`;
- `expires_at` not due;
- run not already cancelling in a way that forbids grant
  (recheck only; full cancel matrix is PR-045).

Equal `(resolution_id, normalized digest)` → idempotent duplicate
(no second apply). Conflict / late / schema-invalid / expired
grant attempt → fail closed.

Expired wins over resolve: if `expires_at <= submitted_at`,
propose `Expired`, not `Resolved`.

Approval grant/deny is **only** `InteractionKind::Approval` plus
the recorded schema (PR-018 fixture shape:
`{"approved": boolean}`). `approved: true` →
`InteractionTerminalOutcome::Granted`. `approved: false` →
`Denied`. Other kinds have no grant bit; their terminal is
`Resolved` without releasing a tool.

Incompatible resolver (schema/version mismatch) suspends
(`SuspendUncertain` / fail closed). Do not guess or silently
cancel.

### 4. Middleware and tool-policy mapping

Runtime maps `StageOutcome::RequestInteraction(request)` to
`KernelInput::RequestInteraction`, **not** to `StageSettled`.
Invalid combo stays `middleware_outcome_not_allowed` at the
middleware port (already true) or `invalid_phase_input` at the
kernel.

`ToolPolicyDecision::RequireApproval` must **stop** being a
synchronous `TOOL_APPROVAL_REQUIRED` synthetic closure. That path
never commits an interaction and is not A02.

At BeforeToolBatch planning:

1. If effective policy is `Deny` → keep synthetic
   `TOOL_POLICY_DENIED` (no interaction).
2. If unknown tool / invalid args → keep those synthetics.
3. If `RequireApproval` **and** `last_interaction_terminal` is a
   `Granted` Approval for this `cursor` → treat as `Allow` and
   emit `ToolBatchPrepared` (protected `call()` may run).
4. If `RequireApproval` otherwise → build
   `InteractionRequest { kind: Approval, policy_component: finstack.policy.approval, ... }`
   with allocated `interaction_id` / `effect_id`, submit
   `RequestInteraction`, do **not** emit `ToolBatchPrepared`.

Denied or expired approval sets `last_interaction_terminal` to
`Denied` / `Expired` and never takes branch 3. No tool
`EffectRequested`, no `call()`.

FR-MW-005 middleware that converts a tool batch into
`RequestInteraction(Approval)` is the same wrapper. Approval
helpers construct `InteractionKind::Approval`; they do not create
a second persistence path.

Update
`catalog_compiles_once_validates_at_one_boundary_and_enforces_approval_floor`
(and any sibling) so the durable floor is suspension + A02, not
the old synthetic code alone. Keep `TOOL_APPROVAL_REQUIRED` as a
stable string only if a denied-approval follow-up diagnostic still
needs it; do not use it as the A02 stand-in.

### 5. InteractionRouter

Replace the PR-014 stub
(`interaction_resolution_unavailable` / always
`IngressRejected`). Mirror `ExternalCompletionRouter` in
`crates/finstack-ai-runtime/src/ingress.rs`.

Locator is `{ tenant_scope, session_id, lane_id, run_id }` +
`Interaction(InteractionId)`. Never scan journals by global
interaction ID. `CommitCoordinator::recover` stays replay-only.

Order (Architecture §10.3 / TDD §12.3):

1. Authenticate and bind any opaque token to the complete locator
   (existing ingress).
2. Recover the named session. Identity/scope/principal mismatch →
   audit-only `unknown_locator` / `scope_mismatch`; no journal
   probe.
3. Unknown outstanding target → audit-only `unknown_target`.
4. Known authorized target: validate schema digest, deadline,
   cancellation, assignee/delegation.
5. Equal `resolution_id` + normalized digest →
   `ExternalRouteOutcome::Idempotent`.
6. Conflict / late / invalid → `Rejected` +
   `RecordExternalCommandRejected` (kind
   `InteractionResolution`).
7. Valid → `KernelInput::InteractionSettled` +
   `Committed`.
8. Required audit failure → fail closed.

`list` is the same locator/principal gate over
`pending_interaction` (0 or 1). It does not scan other sessions.

Reuse allocated settlement IDs when submitting the resolve batch
(same class of bug as PR-042/PR-043 assistant-message / tool
settlement ID reuse). Do not invent a parallel completion machine.

### 6. Resume classifier

Do not reuse `model_resume_action` or `tool_resume_action`.
Mirror the pattern only.

```rust
pub enum InteractionResumeAction {
    NoOutstanding,
    UseRecorded,
    WaitResolution,
    ExpireIfDue,
    SuspendUncertain,
}

pub fn interaction_resume_action(
    state: &KernelState,
    now: Timestamp,
) -> InteractionResumeAction;
```

| Journal / state shape | Action |
| --- | --- |
| no `pending_interaction`, not `AwaitingInteraction` | NoOutstanding |
| terminal already applied / `resolution_identities` hit | UseRecorded |
| `AwaitingInteraction` + pending, `expires_at` due | ExpireIfDue |
| `AwaitingInteraction` + pending, not due | WaitResolution |
| pairing missing / phase-vs-pending mismatch | SuspendUncertain |

First-pass never fabricates a resolution. `ExpireIfDue` submits
`InteractionSettled::Expired` after recover + spawn (clock must
be respawned **before** the fixture deadline, same pitfall as
PR-042). Do not add `TimerFired` or the PR-045 timer adapter.

`CommitCoordinator::recover` stays replay-only. Classify after
recover, after dispatcher install (settlements that resume the
requesting stage can emit the next `ExecuteEffect`), before the
handle is published. Agent does not grow a public “resume”
method. Proofs use `recover` + existing spawn, then router
resolve.

### 7. Timeout and cancelled-while-waiting

`expires_at` on `InteractionRequest` is authoritative. Expire on
the resolve path and on restore-if-due. That is “approval
timeout.”

`InteractionCancelled`: principal+authorization both present for
a principal-initiated cancel; neither for framework
expiry-adjacent / shutdown-authored cancel; one without the other
is invalid (existing DTO). Prove one cancelled-while-waiting
fixture.

Recheck cancel/deadline on resolve. Do **not** add the PR-045
cancel-during-suspended-interaction matrix or native timer
adapter.

### 8. Bindings

Rust: authenticated `list_interactions` / `resolve_interaction`
on the live `Run` handle, implemented by `InteractionRouter`.
Post-crash tests call the router with the same locator.

Python: the same two methods on `Run` (`.pyi`, package export,
hover docs). No Python-owned routing. Pre-beta
`interaction_resolution` shape-normalize stays; it is not the
router.

WASM / browser list/resolve is **PR-048**. A04 is the same
persisted envelope, not a WASM API. Do not add JS/WASM resolve.

No end-user UI. Reference CLI/API examples only if a thin
existing example must show list/resolve; do not build a TUI.

### 9. Kinds

All seven `InteractionKind` variants must be requestable and
resolvable (A01 “any supported”). A04 traces are required for
**approval, choice, and review**. Form, free-text, correction,
and custom may share the same envelope tests (thinner).

Custom stays `InteractionKind::Custom { name }`. Do not add
kind-specific record families.

## Crash / resolution matrix (in-process only)

Not OS kill. Not PR-040 sqlite process-kill. Not PR-048 crash-prefix.
Store: `MemoryJournalStore`. Pattern: `enable_manual_drive` or
park on `AwaitingInteraction`, abort driving tasks, drop owner,
`CommitCoordinator::recover`, router resolve or expire, respawn.

| Row | Scenario | After recover / resolve |
| --- | --- | --- |
| 1 Stop after request | `InteractionRequested` committed; drop | `AwaitingInteraction`; `WaitResolution`; no protected `call()` |
| 2 Valid resolve after stop | later schema-valid resolution | restore `prior_phase`; granting approval may then `ToolBatchPrepared` + one `call()` |
| 3 Denied approval | `approved: false` | `InteractionResolved` + `EffectCompleted`; **no** tool `EffectRequested` / `call()` (A02) |
| 4 Expired | `expires_at` due on resolve or restore | `InteractionExpired` + `EffectFailed`; no tool dispatch (A02) |
| 5 Duplicate | equal `resolution_id` + digest twice | `Idempotent`; no second apply (A03) |
| 6 Conflict | same `resolution_id`, other digest, or late after terminal | fail closed + rejection/audit (A03) |
| 7 Choice envelope | stop + resolve `Choice` | same record family as approval (A04) |
| 8 Review envelope | stop + resolve `Review` | same record family as approval (A04) |
| 9 Cancelled-while-waiting | `InteractionCancelled` while pending | `InteractionCancelled` + `EffectCancelled`; no tool dispatch |
| 10 Kind table | Form / FreeText / Correction / Custom request+resolve | A01 covered; may share one parameterized fixture |
| 11 Uncertain | unpaired / corrupt projection | `SuspendUncertain`; no fabricated resolution |

### Implementation pitfalls (from PR-042/PR-043)

1. Crash tests must `abort()` the driving task, `await` it, then
   `drop(permit)`. Do not leave a task blocked on a oneshot.
2. Spawn `event_task.run()` **before** any resume/resolve
   `submit` (native `EventHubHandle::publish` is oneshot-backed).
3. Reuse allocated settlement IDs on the resolve batch.
4. Install the dispatcher before settlements that can emit the
   next `ExecuteEffect` (grant → `ToolBatchPrepared` → tool
   dispatch).
5. Respawn clocks must stay **before** the fixture `expires_at`.
6. Do not consume `stage_settlements` on request (see locked
   design §1).
7. Do not `ExecuteEffect` the interaction `effect_id`.
8. Do not re-request approval after a grant for the same cursor
   (`last_interaction_terminal`).
9. Public spawn wrappers `Box::pin` inner fns for clippy
   `large_futures`.

## Files

Create later (implementation / candidate, not this planning step):

- `docs/implementation/artifacts/pr-044/README.md`
- `docs/implementation/artifacts/pr-044/candidate-validation.txt`
- `docs/implementation/artifacts/pr-044/security-review.txt`

Modify when implementing:

- `crates/finstack-ai-kernel/src/reducer/input.rs` —
  `RequestInteraction`, `InteractionSettled`
- `crates/finstack-ai-kernel/src/reducer/decide.rs` (or a new
  `reducer/interaction.rs` if decide would otherwise bloat) —
  request/settle decisions, idempotency, schema/assignee/expiry
- `crates/finstack-ai-kernel/src/reducer/apply.rs` —
  `AwaitingInteraction` shapes, interaction bodies,
  `apply_effect_requested` for `EffectKind::Interaction`
- `crates/finstack-ai-kernel/src/reducer/fingerprint.rs` — only
  if new kernel-input fingerprints are required; **no** new
  `ReducerStageOutcome`
- `crates/finstack-ai-kernel/src/state/mod.rs` — v6 projection,
  `pending_interaction`, `resolution_identities`,
  `last_interaction_terminal`
- `crates/finstack-ai-kernel/src/lib.rs` — exports
- `crates/finstack-ai-kernel/tests/model_only_reducer/` — lifecycle
  + pairing + AwaitingInteraction matrix
- `crates/finstack-ai-runtime/src/ingress.rs` — `InteractionRouter`
  success / idempotent / reject; `list`; `known_interaction`
- `crates/finstack-ai-runtime/src/settlement.rs`,
  `coordinator.rs`, `task.rs`, `host_task.rs` — map
  `StageOutcome::RequestInteraction`; park on
  `AwaitingInteraction`; expire-if-due after spawn
- `crates/finstack-ai-runtime/src/tool.rs` — RequireApproval
  suspends via `RequestInteraction`; grant releases once
- `crates/finstack-ai-runtime/src/lib.rs` — exports
  (`InteractionResumeAction`, list/resolve)
- `crates/finstack-ai/src/agent.rs` — agent loop parks on
  `AwaitingInteraction`; continues after restore to `prior_phase`
- `crates/finstack-ai` Run handle — list/resolve wrappers
- `bindings/finstack-ai-python/` — `Run.list_interactions` /
  `Run.resolve_interaction` (Rust, `.pyi`, `__init__.py`, tests)
- `crates/finstack-ai-test/tests/tool_port.rs` — A02 approval
  floor (no dispatch on deny/expire)
- `crates/finstack-ai-test/tests/` — new focused interaction
  runtime fixtures (A01/A03/A04); reuse MemoryJournalStore
- `fixtures/compatibility/public-rust-api/v1/kernel-input/` —
  new request/settle roundtrips
- kernel-state fixtures **only** for interaction-bearing v6
  states; do not rewrite v1–v5 hashes

Do not modify: `docs/planning/`, sqlite store, snapshot DTO
rebuild / settlement-index rebuild, Python/WASM reconcile
(PR-043 exclusion stays), WASM/JS list/resolve, WIT, UI,
timer adapter.

## Proposed tasks

Create ledger rows only at admit. Summaries are
implementation-specific.

1. Tracking — confirm PR-039–PR-043, middleware port,
   ADR-027/025/013, TM-11, exclusions, no branch until
   PR-043 is no longer the sole active PR.
2. Kernel lifecycle — failing tests first for request pairing,
   `AwaitingInteraction`, resolve/expire/cancel, v6 upgrade,
   restored `prior_phase`, unused stage cursor.
3. Router — `InteractionRouter` success path; A03
   duplicate/conflict; audit-only unknown locator; list 0/1.
4. Approval wrapper — middleware/tool policy suspends before
   the protected effect; deny/expire never `call()` (A02);
   grant releases the same cursor once.
5. Crash/resume — A01 stop/resume for supported kinds;
   `interaction_resume_action` table; expire-if-due on restore.
6. Bindings — Rust + Python list/resolve; A04 approval/choice/
   review traces share the envelope.
7. Timeout / cancelled-while-waiting — `expires_at` and one
   cancel fixture; no PR-045 timer/cancel matrix.
8. Validation — A01–A04, TM-11 review, graph checks; stop
   before G5.

## Validation (when authorized)

Focused:

```text
cargo test -p finstack-ai-kernel --offline --locked interaction
cargo test -p finstack-ai-runtime --features native-tokio --offline --locked interaction
cargo test -p finstack-ai-test --test tool_port --offline --locked
cargo test -p finstack-ai --offline --locked interaction
# plus the focused Python list/resolve / envelope test added by this PR
cargo fmt --all --check
cargo clippy --workspace --all-targets --offline --locked -- -D warnings
```

Candidate:

```text
cargo test --workspace --offline --locked
cargo test -p finstack-ai-test --test journal_v1 --offline --locked
cargo test -p finstack-ai-test --test public_rust_api --offline --locked
cargo tree -e normal --locked --offline -p finstack-ai-runtime -p finstack-ai -p finstack-ai-kernel
uv run --no-project python tools/wasm_package/check.py graph
```

Do not require `mise run ci`, the full pytest suite, sqlite kill,
or browser tests to close A01–A04. Do not infer G5.

## Explicit exclusions

No seventh port. No eighth middleware stage. No approval-only
records. No `InteractionRejected`. No `ReducerStageOutcome`
interaction variant (do not consume the stage cursor). No
end-user UI. No PR-045 cancel/timer matrix or timer adapter. No
PR-046/047 lanes. No PR-048 crash-prefix, G5, WASM durable
restart, or WASM list/resolve. No silent ADR-013/027
Implemented/Verified. No exactly-once claim. No fabricated
resolutions. No journal scan by global interaction ID. No
`ExecuteEffect` for the interaction effect. No Python/WASM
reconcile API. Do not reuse the PR-043 envelope. Do not admit
while PR-043 is the sole active logical PR.
