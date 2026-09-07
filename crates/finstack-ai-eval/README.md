# finstack-ai-eval

Evaluation stores freeze a bounded, canonical experiment specification and the
resolved subject locks. Cells expand in task, repetition, then subject order.
Repetition is a sample axis; an infrastructure replacement is a separately
reserved attempt sequence for the same cell.

`MemoryEvalStore` and the optional `SqliteEvalStore` apply the same append-only
contract. Reserve before preparation, bind the actual session/lane before
execution, settle from journal evidence, and append scoring passes separately.
A final subject outcome remains authoritative even if scoring fails. Missing or
uncertain history is `Indeterminate` and cannot admit a replacement. Only
reconciliation proving no admission permits an infrastructure replacement.

SQLite owns a separate database file with `user_version=1`, WAL, and FULL
synchronization. Do not point it at the application journal. Runner ownership
uses an OS file lock, released when the process exits. Both stores bound the
serialized append log to 256 MiB; individual events and frozen specifications
are bounded to 16 MiB. Each attempt permits at most 256 scoring passes.

Run storage and configuration conformance with:

```sh
cargo test -p finstack-ai-eval --features sqlite
```

## Running and resuming

Enable `native-tokio` for `EvalRunner`, subjects and scorers, and `sqlite` for
persistent experiment storage. The default feature set exposes data contracts,
stores, numeric parsing and reporting without a Tokio driver.

Create a validated `EvalSpec`, bind each declaration with
`SharedSubject::new(id, agent, request_template, &spec.tasks)?.bind()`, and pass
those bindings and versioned `Arc<dyn Scorer>` objects to
`EvalRunner::new(spec, store, subjects, scorers)`. `run().await` freezes the spec,
pins resolved locks, reconciles admitted executions and grades final subjects.
`resume().await` follows the same path and skips final work and existing scoring
passes, including failed passes. An application needing dynamic request settings
may implement `Subject::prepare`; preparation must not dispatch an execution.

Each attempt has one actual SDK session and lane. Reservations precede preparation;
the execution identity is persisted before `Lane::run`. Journal reconciliation
covers child lineage and deduplicates effect receipts by run/effect identity.
Missing history, retained-prefix loss, active children and uncertain effects are
visible as unresolved or incomplete evidence. No replacement follows from a
zero-token observation alone.

Call `cancel()` and await the run to observe settlement. A dropped run await
leaves a tracked owner holding the store lease; it does not implicitly cancel
admitted durable work. Timeout/cancellation requests SDK cancellation, joins
local driving within its settlement grace, then classifies journal evidence.
Unresolved external effects remain unresolved.

A finite monetary budget needs a unit and compatible accepted pricing policy on
every bound subject/grader agent. The runner checks complete recorded spending
between admissions. Concurrent in-flight work can exceed that threshold.
Missing/mixed-unit cost never becomes zero or a fabricated converted total.

## Scorers and rescoring

Built-ins are `ExactMatchScorer`, `IncludesScorer`, `RegexScorer`,
`NumericToleranceScorer`, `StructuredFieldScorer`, `RecordKindsScorer`, and
`JudgeScorer`. `ScoreMicros` is an integer in `0..=1_000_000`. Numeric scorers
parse exact bounded decimal values with explicit unit equivalence and
full/partial tolerance bands. Structured scorers use configured JSON pointers.
A custom `Scorer` receives optional output: subject failure is not a scorer
exception. Scorer errors append their own failed pass and never reclassify the
subject outcome.

A judge uses host-defined choice labels mapped to integer grades and a strict
output schema. Task, target and answer are JSON-quoted untrusted evidence; no
transcript or harness state is supplied. Configured toolsets and capability
catalogs are rejected by default. `JudgeRubric::allow_tools` explicitly opts in.
Grader execution has its own pre-dispatch reservation, actual session, resolved
lock and request digest. Grader spending survives a failed scorer or a crash
between grader completion and score persistence, and counts against the same
experiment budget while remaining separate in reports.

`rescore().await` appends new passes using recorded subject journals and current
bound scorer versions. It never invokes subject preparation or subject models.
Judge rescoring may execute new graders and incur recorded spend. A missing
subject journal produces `eval_rescore_session_missing` in a failed scoring pass.
An unfinished grader must reconcile under its original version and lock before
another pass is permitted. Old passes remain available in the store/export.

## Reports, gates and exports

`EvalReport::from_snapshot(&run.snapshot)` is pure: it reads no journal and runs
no scorer. Each exact `(subject, scorer, scorer_version, score_name)` uses only
the latest pass of that scorer per cell. Versions never mix. Repetitions reduce
within each task using mean, unbiased pass@k, or at-least-k; then task reductions
produce mean and sample standard error. One task has no estimable stderr.
Partial observations may have descriptive statistics, with complete-task and
cell coverage shown alongside them.

The first declared subject is the paired baseline. Each other arm is compared
at equal task/repetition coordinates. Quality deltas use raw cell grades;
usage deltas require complete measured tokens; cost deltas require complete
cost in the same unit. Pair counts and missing coverage are explicit. Costs use
exact signed integer sums; derived statistical means/stderr are approximate.

`ThresholdGate::evaluate` checks a selected metric's minimum reduced mean and/or
maximum paired mean regression. It returns independent quality, infrastructure,
scoring and incomplete-coverage reasons. Current experiment-wide infrastructure
or scoring failures prevent a passing gate. Historical superseded failures stay
in accounting but do not masquerade as current failures. Missing metric versions
or pairs cannot pass the gate.

`export_jsonl(&snapshot, writer)` writes one row per authoritative attempt, with
all score passes and associated graders. It omits task/input/target bodies,
transcripts, arbitrary score metadata and explanations. Integer measurements
and monetary micros are decimal strings. `report.write_json(writer)` writes the
summary, including locks, counts, spending, aggregates and paired comparisons;
statistical numbers are decimal strings too. Treat numeric strings as decimal
or integer values when loading them in Python/TypeScript. TypeScript execution
is deferred; JSONL/summary consumption is the supported integration.

Validate all four Rust slices, including real process restart, using:

```sh
mise run test-eval
```
