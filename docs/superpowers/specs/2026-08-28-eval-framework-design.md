# Evaluation Framework (`finstack-ai-eval`) — Design

**Date:** 2026-08-28
**Status:** Proposed design, pre-implementation
**Path:** `crates/finstack-ai-eval`

## 1. Problem

finstack-ai can execute, record, and deterministically replay agent runs, but
it has no machinery for asking "how well does this agent perform on this set of
tasks, and did it get better or worse?" Concretely:

- `AgentRunOutput` (crates/finstack-ai/src/agent/types.rs:173) carries no
  usage, cost, or duration; token/cost data exists only durably per effect
  (`EffectCompleted.usage`, crates/finstack-ai-kernel/src/effects/lifecycle.rs:305)
  and in observers (`BillingObserver`). Nothing aggregates it per task, per
  subject, or per experiment.
- There is no notion of a task set, a repetition, a score, or an experiment.
  `finstack-ai-test` provides scripted doubles and golden-trace conformance —
  correctness fixtures with checked-in expectations — not statistical
  evaluation of open-ended agent behavior.
- There is no regression story: no baseline comparison, no pass@k, no way to
  gate a provider/prompt/middleware change on "did quality drop".

The workloads this layer must serve first are financial knowledge tasks —
risk-and-merits analysis, business descriptions, financial modelling, and
document Q&A. That shapes the built-in scorer set (§5): financial answers
are numbers in varied costume ("$1.2bn", "1,200.0", "119.7 bps"), and
modelling outputs are structured values, so numeric-tolerance and
structured-field scoring ship in v1 alongside exact/regex/judge scoring.

Every mature agent stack has grown this layer (survey in §2). The workspace is
unusually well positioned to build a stronger one than the field standard: the
journal is a complete, digest-chained record of every run, so an evaluation
result can be a *projection over recorded history* — re-scorable after the
fact without re-executing — rather than a lossy log written beside it.

## 2. State of the art, and what we adopt

Surveyed 2026-08-28: Inspect AI (UK AISI), EleutherAI lm-evaluation-harness,
OpenAI Evals (legacy + hosted API), Stanford HELM, promptfoo, Braintrust,
DeepEval, RAGAS, LangSmith, W&B Weave, SWE-bench (+Verified), terminal-bench,
METR Vivaria, OpenHands, and Apache Maka's `packages/eval`.

The field has converged on one skeleton: **a dataset of samples (input /
target / metadata) × a subject-under-test → one result record per sample →
scorers emitting named scores in [0,1] with an explanation → aggregation
(mean/stderr/pass@k) → experiment compared against a baseline**, with
repetition as a first-class axis and append-only/immutable result storage.

Adopted, with sources:

| Idea | Source | Why it fits here |
|---|---|---|
| Experiment expands to **cells** = task × repetition × subject; all subject arms of a repetition are peers (paired comparison) | Maka `expandExperiment()` | Maps 1:1 onto our Session-per-cell isolation; enables paired subject diffs |
| **Repetition ≠ retry**: a repetition is a new sample; an infra retry appends a *replacement attempt* to the same cell | Maka | Keeps statistics honest; our journal supplies the evidence for the distinction |
| **Earliest-valid-attempt authority**: the lowest-sequence non-replaceable attempt is the cell's result, permanently | Maka `selectCellResult` | Makes cherry-picking a luckier rerun structurally impossible |
| Evidence-based failure attribution: admitted model usage ⇒ `SubjectFailed`, never a free retry | Maka metering rule | We have *stronger* evidence than Maka: committed `EffectCompleted` records |
| Result kernel limited to score, normalized usage, cost, duration, status, artifact refs | Maka `EvalResult` | Everything else stays in the journal; no second copy of transcripts |
| Repetition reducers: `mean`, `pass_at_k`, `at_least_k` | Inspect `Epochs` | Minimal useful set; closed-form stderr like lm-eval |
| Judge = **constrained classification** (CoT → choice → fixed score map), never raw numeric scoring; grader model pinned; injection-hardened delimiters | Inspect `model_graded_qa`, Braintrust `LLMClassifier` | Our schema-verified structured output (`RunResult::try_from_committed`) enforces the choice contract cryptographically |
| Resume = skip cells that already have an authoritative result; frozen spec re-open fails on digest mismatch | Inspect `eval_set`, Maka frozen `experiment.json`, terminal-bench `RunLock` | Same shape as our idempotent, fail-closed store culture |
| Results as self-contained exportable files (JSONL), not a hosted DB | Inspect `.eval`, lm-eval, Maka | Matches `BillingObserver::export_jsonl` precedent; UI project consumes files or the SQLite store |

Deliberately rejected:

- **Solver/prompt-orchestration DSL inside the harness** (Inspect solvers,
  HELM adapters). Our subject is a composed `Agent`; prompting is the
  agent's own configuration. The harness evaluates whole subjects
  (Maka/terminal-bench school), because that is what the SDK produces.
- **Model-call response caching** as the reproducibility lever (promptfoo,
  HELM, lm-eval). We have something stronger: the journal. Re-scoring reads
  recorded runs; re-execution is an explicit new attempt. A provider-level
  HTTP cache would blur the attempt/repetition boundary and belongs, if
  ever, in a provider leaf — not here.
- **Hosted-service features** (annotation queues, online scoring,
  dashboards). The UI is a separate project; this crate exposes data
  (store + JSONL + report types), not screens.
- **Per-cell container/VM provisioning** (SWE-bench images, OSWorld
  snapshots). Environment isolation stays a subject concern via existing
  toolsets (`finstack-ai-sandbox-e2b`, process confinement). See §9.

## 3. Decisions

1. **One crate, `crates/finstack-ai-eval`, hosts tier.** It composes the
   public SDK (`finstack_ai::Agent`), so it cannot live under `extensions/`
   (tier 2) or as a named composition crate (same-tier edges are forbidden
   outside `extensions`/`hosts` — scripts/ci/check_layering.py:70). Any
   `crates/` directory not named in `EXACT_TIERS` defaults to `HOSTS`
   (check_layering.py:52-57), the same position the bindings occupy: a
   consumer of the public surface. No layering-script change. Native-only:
   added to `FORBIDDEN_WASM` (scripts/wasm_package/check.py).
2. **No new port, no new record kind, no protocol change.** The kernel
   record vocabulary (`RecordBody`, ~45 variants) and the six ports are
   untouched — anything else is an ADR-gated hard stop
   (.agents/rules/01-engineering-conformance.md "Hard stops"). Eval state
   lives *outside* the journal in its own store, referencing runs by
   `OperationLocator`/`SessionId`; the journal remains the sole authority
   for run content. This is the workflow-worker precedent: adapter tables
   beside the journal, "the kernel journal stays authoritative".
3. **The harness drives runs through `Agent::start`, never `Agent::run`.**
   `run` closes the event subscription before awaiting the result
   (agent/handle.rs:621); the harness needs `next_event_batch()` for live
   capture. Capture therefore requires **no observer injection** and works
   with any `Agent` a subject returns — the SDK's interactive subscription
   (capped at `Sensitivity::Confidential`) is sufficient because scores,
   usage, and durations never need `Credential` payloads.
4. **Authoritative measurement comes from the journal, not the live
   stream.** After a run terminates (or when re-attaching after a crash),
   the harness loads the session (`Agent::journal_store()` +
   `inspect_session`, crates/finstack-ai-runtime/src/services/session_inspect.rs)
   and derives: usage/cost by folding `EffectCompleted.usage` **deduplicated
   by `EffectId`** (at-least-once settlement delivery is why
   `BillingObserver` documents a double-count caveat; the fold must not
   inherit it), duration from `RunAccepted` → terminal record timestamps,
   and the structural trace from committed record kinds. The live event
   stream is a progress optimization only.
5. **Scores are integer micros.** `ScoreMicros` is `u32` in
   `0..=1_000_000`, the score analog of `CostAmount::micros`
   (crates/finstack-ai-kernel/src/primitives/usage.rs): no floats in
   digested or exported data, exact cross-binding equality, JCS-canonical
   without float hazards. Aggregates (mean, stderr) are derived `f64` in
   reports only and exported as decimal strings, mirroring
   `BillingObserver::export_jsonl`.
6. **The attempt record is a result kernel, not a transcript.** An
   `AttemptRecord` stores scores, usage, cost, duration, status, failure
   code, the run's `OperationLocator`, the session id, `record_kinds`, and
   artifact references — never message bodies. Transcripts already exist,
   once, digest-chained, in the journal; duplicating them into a second
   store would create a second redaction surface and a second retention
   problem. Exports contain no message content in v1.
7. **Statuses and the authority rule are Maka's, verbatim in semantics.**
   `AttemptStatus::{Completed, SubjectFailed, InfraFailed, Indeterminate}`.
   `InfraFailed`/`Indeterminate` attempts are replaceable; `Completed`/
   `SubjectFailed` are final. A cell's authoritative attempt is the
   earliest non-replaceable one. Classification is evidence-based: if the
   journal shows committed model usage for the attempt's session, a dead
   worker yields `SubjectFailed`, not `InfraFailed`. `RunStatus::Faulted`
   being terminal and lock-poison-safe (`RunLifecycle`,
   crates/finstack-ai-runtime/src/exec/run_types.rs:79) makes the observed
   terminal state trustworthy input to this classification.
8. **Experiment identity is a frozen, digest-checked spec.** `EvalSpec` is
   schema-versioned serde data (`"finstack.eval.v1"`), canonicalized with
   the workspace JCS canonicalizer and digested with domain separation
   (domain `eval-spec`). `EvalStore::freeze` persists spec + digest +
   engine version; re-opening with a differing digest fails closed
   (`EVAL_SPEC_DIVERGED`) — Maka's frozen `experiment.json`, our codes.
   Each subject's `ResolvedAgentLock` (crates/finstack-ai/src/bundle/lock.rs:106)
   is recorded at first resolution: credential-free, exact-reconstruction
   metadata beside every result.
9. **Subjects are code, declared by id in the spec.** A `Subject` yields an
   `Agent` plus a run-request template per cell. Model handles, stores, and
   credentials cannot be data; the spec pins subject *identity* (id,
   expected lock digest once known) and the runner binds ids to `Subject`
   implementations at startup, failing closed on missing or mismatched
   bindings. `SharedSubject` wraps one prebuilt `Agent` (resolve once,
   retain handles — the ordinary-turn rule); per-cell construction is the
   implementor's choice when toolset state demands isolation.
10. **One cell = one fresh `Session`.** Cells are isolated by session
    scoping in the journal store, which also gives each attempt an
    unambiguous evidence trail and makes cross-target re-scoring trivial.
    Child runs spawned by the subject stay inside the cell's lineage
    (`ChildRunPrepared` records), so subagent fan-out is scored as part of
    the cell.
11. **Scorers run after the attempt and may themselves be agents.**
    `Scorer::score(ScoreContext) -> Vec<Score>` receives the sample
    (input/target/metadata), the terminal output, and journal-derived
    measurement — full transcript access via the store handle, not a copy.
    `JudgeScorer` composes an ordinary grader `Agent` with a rubric of
    choices mapped to `ScoreMicros`, forced through schema-verified
    structured output; grader runs are themselves journaled sessions, so
    judge behavior is auditable and re-scorable like everything else.
    Judge input embeds subject output inside hardened delimiters and the
    grader agent carries no tools unless the rubric explicitly adds them.
12. **Re-scoring recorded runs is a first-class operation.**
    `score_recorded(store, session, sample, scorers)` produces a new
    `ScoreSet` over an existing attempt without re-execution, keyed by
    `scorer_id` + `scorer_version`; score sets append, never overwrite.
    This is the capability no surveyed framework has (their caches
    approximate it; our journal guarantees it) and the primary reason eval
    belongs in this codebase rather than beside it.
13. **The store follows workspace SQLite law.** `EvalStore` trait with
    `MemoryEvalStore` and `SqliteEvalStore`. The SQLite store owns its
    database file, so it versions with `PRAGMA user_version = 1`
    (.agents/rules/01, schema-versioning law; `finstack-ai-store-sqlite`
    precedent) — it never shares a file with the journal in v1, avoiding
    the `<prefix>_schema` adapter regime entirely. Append-only attempts
    with enforced per-cell sequence; WAL + `synchronous=FULL` for the
    acknowledged durable mode, matching store-sqlite.
14. **The runner is an in-process batch scheduler, not a workflow
    daemon.** Bounded-concurrency attempt execution over expanded cells
    (tokio semaphore), per-attempt timeout from the spec, budget check
    between attempts (stop scheduling when the folded cost of admitted
    attempts exceeds `budget_micros`), and resume-by-skip on restart. It
    deliberately does not reuse `WorkflowWorker` (cron/wake/lease machinery
    solves multi-process coordination eval v1 does not have); the report
    type mirrors `TickReport`'s counter style. A distributed runner over
    the session-server protocol is excluded (§9) — `remote.rs` is frozen
    and eval must not force protocol changes.
15. **Aggregation is small and honest.** Per (task, subject, scorer):
    reduce repetitions by `mean`, `pass_at_k` (needs a pass threshold on
    the score), or `at_least_k`; then across tasks: mean with closed-form
    stderr and n. Paired subject comparison re-pairs cells by
    (task, repetition), drops unmatched pairs, and reports per-pair deltas
    — Maka's documented method, implemented rather than left to the
    reader. No bootstrap, no clustered stderr in v1 (§9).

## 4. Trust and threat mapping

Eval runs execute the same trust surfaces as ordinary runs (tool policy,
process confinement, net-guard, HITL) — nothing here weakens them. New
surfaces introduced by this crate:

| Control | Owner |
|---|---|
| Attempt records contain no message bodies, no secret material; locators/digests only | `AttemptRecord` construction (type makes it unrepresentable) |
| JSONL export excludes transcripts; numerics as decimal strings | `export_jsonl` |
| Judge prompt-injection: subject output embedded as data inside hardened delimiters; grader agent has no tools by default; grade extracted only via schema-verified structured output | `JudgeScorer` |
| Grader misuse of context: judge sees exactly the rubric, sample target, and subject output — not the parent harness state | `JudgeScorer` template |
| Spec divergence after freeze fails closed (`EVAL_SPEC_DIVERGED`) | `EvalStore::freeze`/`open` |
| Subject binding mismatch (unknown id, lock digest changed when spec pins one) fails closed before any attempt | `EvalRunner::preflight` |
| Duplicate settlement delivery cannot double-count usage/cost | journal fold dedupes on `EffectId` |
| Store integrity: append-only attempts, enforced sequence, `user_version` fail-closed on unknown schema | `SqliteEvalStore` |
| Budget stop is between attempts and never cancels an admitted run mid-flight (admitted work is scored, echoing commit-before-effect) | `EvalRunner` |

## 5. Public surface

```rust
pub use config::{EvalSpec, EvalLimits, RepetitionReducer, SubjectDecl, TaskSample, TaskSet};
pub use cell::{Cell, CellId, CellOutcome};
pub use attempt::{AttemptRecord, AttemptSequence, AttemptStatus, MeasuredUsage};
pub use score::{Score, ScoreContext, ScoreMicros, ScoreSet, Scorer, ScorerId};
pub use judge::{JudgeRubric, JudgeScorer};
pub use scorers::{ExactMatchScorer, IncludesScorer, NumericToleranceScorer, RecordKindsScorer, RegexScorer, StructuredFieldScorer};
pub use scorers::numeric::{ParsedNumber, ToleranceBands};
pub use subject::{SharedSubject, Subject, SubjectBinding, SubjectId};
pub use store::{EvalStore, FrozenExperiment, MemoryEvalStore};
#[cfg(feature = "sqlite")]
pub use store::sqlite::SqliteEvalStore;
pub use runner::{EvalRunner, EvalRunReport, RunnerConfig};
pub use report::{Aggregate, EvalReport, PairedComparison, ThresholdGate};
pub use rescore::score_recorded;
pub use error::{EvalError /* codes below */, EvalStoreError};
```

Core types (signatures normative, bodies illustrative):

```rust
/// Score in integer micros: 0 ..= 1_000_000 maps to [0.0, 1.0].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(transparent)]
pub struct ScoreMicros(u32);          // constructor validates the bound: EVAL_SCORE_OUT_OF_RANGE

pub struct Score {
    pub scorer: ScorerId,             // stable snake_case id
    pub scorer_version: u32,
    pub name: Arc<str>,               // one scorer may emit several named scores
    pub value: ScoreMicros,
    pub passed: Option<bool>,         // set when the scorer applies a threshold
    pub explanation: Option<Arc<str>>,
    pub metadata: Option<RawJson>,
}

pub trait Scorer: Send + Sync {
    fn id(&self) -> ScorerId;
    fn version(&self) -> u32;
    fn score<'a>(&'a self, ctx: &'a ScoreContext<'a>)
        -> Pin<Box<dyn Future<Output = Result<Vec<Score>, EvalError>> + Send + 'a>>;
}

pub struct ScoreContext<'a> {
    pub sample: &'a TaskSample,                  // input, target, metadata, attachments
    pub output: &'a AgentRunOutput,              // absent for terminal-failure attempts
    pub usage: &'a MeasuredUsage,                // journal-derived, EffectId-deduped
    pub inspect: &'a SessionInspectSnapshot,     // phase, head, result text
    pub journal: &'a Arc<dyn JournalStore>,      // full-transcript access on demand
    pub session_id: &'a SessionId,
}

pub trait Subject: Send + Sync {
    fn id(&self) -> SubjectId;
    fn prepare<'a>(&'a self, cell: &'a Cell)
        -> Pin<Box<dyn Future<Output = Result<PreparedAttempt, EvalError>> + Send + 'a>>;
}
pub struct PreparedAttempt {
    pub agent: Agent,
    pub request: AgentRunRequest,     // subject owns model name, settings, security, timeout
}

pub struct AttemptRecord {
    pub cell: CellId,                 // "{task_id}::{repetition}::{subject_id}"
    pub sequence: AttemptSequence,    // 1-based, append-only per cell
    pub status: AttemptStatus,        // Completed | SubjectFailed | InfraFailed | Indeterminate
    pub failure_code: Option<Arc<str>>,
    pub locator: Option<OperationLocator>,
    pub session_id: SessionId,
    pub usage: MeasuredUsage,         // input/output/total tokens, cost micros+unit, effects, uncosted
    pub duration_ms: u64,             // RunAccepted -> terminal record timestamps
    pub record_kinds: Arc<[Arc<str>]>,
    pub scores: Vec<ScoreSet>,        // append-only; one per (scorer_id, scorer_version) pass
    pub artifacts: Arc<[ArtifactRef]>,
    pub started_at_ms: u64,
    pub completed_at_ms: u64,
}
```

The two financial scorers (workloads in §1):

- **`NumericToleranceScorer`** — parses the sample target and the subject's
  answer into `ParsedNumber { mantissa: i128, exponent: i32, unit }` (exact
  decimal, no floats, matching the workspace's integer-money culture),
  normalizing currency symbols, thousands separators, scale suffixes
  (`k`/`m`/`mm`/`bn`/`billion`), and percent/bps/fraction equivalence
  (`119.7 bps` ≡ `1.197%` ≡ `0.01197`). Scores by `ToleranceBands
  { full_within_ppm, partial_within_ppm: Option, partial_micros }` —
  full credit inside the tight band, optional partial band, else zero;
  `passed` reflects the full-credit band. Relative tolerance is
  parts-per-million (integer), absolute tolerance a `ParsedNumber`.
- **`StructuredFieldScorer`** — for financial-model outputs: parses the
  sample target as JSON (non-JSON target → score-set error
  `EVAL_TARGET_INVALID`), reads the subject's schema-verified
  `structured_json()`, and compares a configured list of `FieldSpec
  { pointer /* RFC 6901 */, tolerance: Exact | NumericPpm(u64), weight }`.
  Emits one named score per field (name = pointer; missing field scores
  zero) plus a weighted `aggregate` score. Field-level results make
  regressions legible ("FY26 FCF drifted, everything else held").

Error codes (workspace convention — `pub const` beside the enum, stable across
bindings): `EVAL_SPEC_INVALID`, `EVAL_SPEC_DIVERGED`, `EVAL_SUBJECT_UNBOUND`,
`EVAL_SUBJECT_LOCK_MISMATCH`, `EVAL_SCORE_OUT_OF_RANGE`,
`EVAL_ATTEMPT_SEQUENCE_CONFLICT`, `EVAL_CELL_FINALIZED`,
`EVAL_BUDGET_EXHAUSTED`, `EVAL_JUDGE_OUTPUT_INVALID`, `EVAL_TARGET_INVALID`,
`EVAL_STORE_UNAVAILABLE`, `EVAL_RESCORE_SESSION_MISSING`.

## 6. Spec and export formats

`EvalSpec` (serde, JCS-canonicalized for the freeze digest, domain `eval-spec`):

```json
{
  "schema_version": "finstack.eval.v1",
  "name": "coding-regression-suite",
  "tasks": [
    {"task_id": "fix-linked-ctor", "input": "…", "target": "…",
     "metadata": {"category": "wasm"}, "attachments": []}
  ],
  "subjects": [
    {"subject_id": "main-agent", "lock_digest": null},
    {"subject_id": "candidate-agent", "lock_digest": null}
  ],
  "repetitions": 5,
  "scorers": ["exact_match", "judge_correctness"],
  "reducer": {"kind": "pass_at_k", "k": 2, "pass_threshold_micros": 1000000},
  "limits": {"attempt_timeout_ms": 120000, "max_replacement_attempts": 2,
             "max_concurrency": 4, "budget_micros": null, "budget_unit": "USD"}
}
```

Rules: `repetitions >= 1`; `subject_id`/`task_id`/scorer ids are non-empty,
unique, `[a-z0-9_-]+`; `lock_digest`, when present, must match the bound
subject's `ResolvedAgentLock` digest at preflight. Task inputs above the
attachment threshold ship as `ArtifactRef`s (run attachments already refuse
raw bytes; `MAX_RUN_ATTACHMENTS = 8` applies per attempt).

Export: one JSONL row per authoritative attempt (the result kernel, cost as
decimal string) plus a `summary.json` (aggregates, paired comparisons, spec
digest, engine version, subject lock digests). No message bodies anywhere.

## 7. Attempt pipeline (order is normative)

1. **Preflight.** Open-or-freeze spec against the store (digest check, fail
   closed) → bind subjects (fail closed on unbound id) → resolve/record
   `ResolvedAgentLock` per subject → expand cells → list existing attempts
   → skip cells with an authoritative attempt; queue replacements for
   replaceable ones up to `max_replacement_attempts`.
2. **Admit.** Budget check over folded admitted cost; reserve the next
   attempt sequence in the store (`EVAL_ATTEMPT_SEQUENCE_CONFLICT` on race
   — one writer per experiment, enforced, not assumed).
3. **Execute.** `subject.prepare(cell)` → `Session::create` (fresh, per
   attempt) → `agent.start(request)` → drain `next_event_batch()` for
   progress until terminal → `run.result().await` under the spec timeout.
4. **Measure.** Load the session from the journal; fold usage/cost deduped
   by `EffectId`; duration from record timestamps; capture `record_kinds`,
   locator, terminal phase via `inspect_session`.
5. **Classify.** Runner/harness fault with zero admitted usage →
   `InfraFailed` (replaceable). Any committed usage or any terminal
   subject outcome (including timeout, limit, refusal, `Faulted`) →
   `Completed` (result present) or `SubjectFailed` — final either way.
   Ambiguous journal state after crash → `Indeterminate` (replaceable).
6. **Score.** Skip scorers for `InfraFailed`/`Indeterminate`. Run every
   configured scorer; judge scorers execute their grader agent in a
   dedicated grader session. A scorer error fails the *scoring pass*
   (`ScoreSet` records the error code), never the attempt's status.
7. **Persist.** Append the `AttemptRecord` (append-only; sequenced), then
   mark the cell outcome by the authority rule. Persist before scheduling
   the next attempt for the same cell.
8. **Report.** After the queue drains: aggregate, pair, export, and return
   `EvalRunReport { cells_total, attempted, skipped_resumed, completed,
   subject_failed, infra_failed, indeterminate, replacements, budget_spent,
   duration }` — `TickReport`-style counters.

Re-scoring (`score_recorded`) enters at step 4 against an existing session
and appends a `ScoreSet` to the existing attempt (step 6–7), never a new
attempt.

## 8. Composition (who calls what)

```
user code ──► EvalRunner::new(spec, bindings, store, RunnerConfig)
                 │  preflight: freeze/verify spec, bind subjects, record locks
                 ▼
              cells (task × repetition × subject)
                 │  bounded concurrency
                 ▼
              Subject::prepare ──► Agent::start ──► AgentRun events/result
                 │                        │
                 │                        ▼
                 │             JournalStore (authoritative transcript)
                 ▼                        │
              measure + classify ◄────────┘   (EffectId-deduped fold, inspect_session)
                 ▼
              Scorer::score  (JudgeScorer ► grader Agent ► its own session)
                 ▼
              EvalStore (attempts, append-only) ──► EvalReport / JSONL / gate
```

The UI project consumes `SqliteEvalStore` files and JSONL exports; it never
needs this crate at runtime. `finstack-ai-test`'s `ScriptedModel` composes as
an ordinary subject model for the crate's own tests and for users' offline
smoke evals — via dev-dependency, per workspace norm.

## 9. Explicit exclusions

Tracked here so their absence is a decision, not an oversight:

1. **Python/TypeScript bindings** — deferred with a tracked follow-up
   (binding-parity rule allows explicit deferral). The store schema and
   JSONL are language-neutral; Python analysis needs no binding to read
   them. Driving evals from Python arrives with the deferral.
2. **Distributed runner** — no protocol work; `remote.rs` is frozen. A
   future runner models workers as ordinary hosts owning local journals
   and reporting results out-of-band.
3. **Environment provisioning** (containers/VM snapshots per cell) —
   subject responsibility via existing toolsets (`sandbox-e2b`, process
   confinement). Revisit only with a concrete benchmark need.
4. **Bootstrap/clustered stderr, Krippendorff's alpha, judge calibration
   loops** (RAGAS `metric.train`) — v1 ships mean/stderr/pass@k; the
   attempt store retains everything needed to add statistics later
   without re-running.
5. **Pairwise judge scoring** (LangSmith `evaluate_comparative`) — paired
   *numeric* comparison ships in v1; a pairwise *judge* is a later scorer.
6. **Claim-level faithfulness scorer** (RAGAS-style: decompose the answer
   into claims, verify each against the source document) — buildable today
   as a custom judge-pipeline `Scorer`; promoted to a built-in once the
   rubric stabilizes on real risk/merits workloads.
7. **Human annotation queues / UI** — separate project by design.
8. **Model-call caching** — rejected (§2); re-scoring covers the cost
   concern for scorer iteration.
9. **Cron-scheduled evals** — compose `workflow-local`'s cron with a
   host-side call once needed; no coupling now.

## 10. Testing

Per `.agents/rules/02`: deterministic, offline, focused on the changed
invariant.

- **Spec/config:** serde round-trips; JCS digest stability fixture
  (`fixtures/compatibility/eval/v1/valid--spec-minimal.json`,
  `valid--spec-full.json`, `invalid--*` per validation rule); freeze/reopen
  divergence fails closed.
- **Expansion/authority:** cell-id determinism and ordering; earliest-valid
  attempt selection truth table across all status × sequence combinations;
  replacement-cap enforcement.
- **Measurement:** journal fold with duplicate `EffectCompleted` delivery
  (must count once — the regression `BillingObserver` documents); usage
  totals vs a `ScriptedModel` session with known usage; duration from
  record timestamps under `FixedClock`.
- **Classification:** evidence table — dead worker with/without admitted
  usage, timeout, `Faulted`, cancellation → expected status; crash-prefix
  style restart test resuming a half-run experiment (skip + replace).
- **Runner:** bounded concurrency observed via `ScriptedModelControl`
  gates; budget stop between attempts (admitted attempt still scored);
  per-attempt timeout produces `SubjectFailed`, not `InfraFailed`.
- **Scorers:** programmatic scorer goldens; numeric-parse golden table
  (`"$1.2bn"` ≡ `1_200_000_000`; `"119.7 bps"` ≡ `"1.197%"`; separators,
  negatives-in-parentheses, mixed units rejected) and band-boundary cases
  at exact ppm edges; structured-field weighted aggregate, missing-field
  zero, non-JSON target → `EVAL_TARGET_INVALID` isolated to the score set;
  `JudgeScorer` against a
  `ScriptedModel` grader emitting valid/invalid/malicious structured
  output (`EVAL_JUDGE_OUTPUT_INVALID` path, delimiter hardening —
  subject output containing grader instructions must not alter the grade);
  scorer error isolates to the `ScoreSet`.
- **Store:** memory/sqlite conformance parity suite; append-only and
  sequence-conflict; `user_version` mismatch fail-closed; WAL durable
  mode; JSONL export golden (decimal strings, no message bodies —
  asserted by construction *and* by scanning the export for a sentinel
  transcript string).
- **Re-scoring:** `score_recorded` over a recorded fixture session equals
  live scoring of the same run; appended score sets never mutate priors.
- **End-to-end:** two scripted subjects × 3 tasks × 2 repetitions through
  the full pipeline to a golden `summary.json` (checked-in expectation,
  never rewritten — no snapshot auto-accept).

## 11. Repo integration checklist

- Workspace member `crates/finstack-ai-eval`; `version.workspace = true`,
  `[lints] workspace = true`, the standard lint prelude byte-for-byte;
  deps only from `[workspace.dependencies]`.
- Features: `default = []`, `sqlite` (rusqlite via workspace pin),
  `native-tokio` alignment with the facade's default.
- `scripts/ci/check_layering.py`: no change (defaults to hosts) — verify
  with `mise run check-layering`.
- `scripts/wasm_package/check.py`: add to `FORBIDDEN_WASM`.
- `cargo-public-api` baseline added in the same change
  (`fixtures/compatibility/public-rust-api/cargo-public-api/`).
- `fixtures/compatibility/eval/v1/` with `README.md` and `valid--`/
  `invalid--` naming.
- Crate `README.md`, rustdoc with `# Errors` on every fallible public fn,
  CHANGELOG entry, `mise run ci-rust` green.
- Tracked deferral filed for Python/TS binding parity (§9.1).
