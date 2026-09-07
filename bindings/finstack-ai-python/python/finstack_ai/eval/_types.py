"""Typed wire projections of the Rust evaluation contracts."""

from typing import Literal, TypeAlias, TypedDict

from ..artifacts import ArtifactReference
from ..callbacks import JsonValue, ModelSettings

AttemptStatus: TypeAlias = Literal[
    "completed", "subject_failed", "infra_failed", "indeterminate"
]
Reconciliation: TypeAlias = Literal["no_admission", "terminal", "unresolved"]


class Cell(TypedDict):
    """Frozen task × repetition × subject coordinate."""

    id: str
    task_id: str
    repetition: int
    subject_id: str


class TaskSampleData(TypedDict):
    """Scorer input from the frozen dataset; omitted from exports."""

    task_id: str
    input: str
    target: str
    metadata: JsonValue
    attachments: list[ArtifactReference]


class MeasuredCost(TypedDict):
    """Exact cost micros in the store/callback projection (exports use strings)."""

    unit: str
    micros: int


class MeasuredUsage(TypedDict):
    """Journal-derived parent/child measurement, with explicit coverage."""

    input_tokens: int | None
    output_tokens: int | None
    total_tokens: int | None
    cost_by_unit: dict[str, int]
    cost: MeasuredCost | None
    effects: int
    model_effects: int
    uncosted_effects: int
    complete: bool


class Score(TypedDict):
    """Exact Rust score; value is integer millionths in 0..=1_000_000."""

    scorer: str
    scorer_version: int
    name: str
    value: int
    passed: bool | None
    explanation: str | None
    metadata: JsonValue


class ScoreSet(TypedDict):
    """Append-only scoring pass, separate from subject classification."""

    scorer: str
    scorer_version: int
    scores: list[Score]
    failure_code: str | None


class ExecutionIdentity(TypedDict):
    """Actual session/lane identity persisted before dispatch."""

    tenant_scope: str
    session_id: str
    lane_id: str


class OperationLocator(ExecutionIdentity):
    """Committed run locator, including tenant authority scope."""

    run_id: str


class AttemptReservation(TypedDict):
    """Pre-dispatch reservation; repetition and replacement sequence are separate."""

    cell: Cell
    sequence: int
    started_at_ms: int
    execution: ExecutionIdentity | None


class AttemptRecord(TypedDict):
    """Subject outcome and journal-derived measurements, excluding graders."""

    cell: str
    sequence: int
    status: AttemptStatus
    reconciliation: Reconciliation
    failure_code: str | None
    locator: OperationLocator | None
    usage: MeasuredUsage
    duration_ms: int | None
    record_kinds: list[str]
    scores: list[ScoreSet]
    artifacts: list[ArtifactReference]
    started_at_ms: int
    completed_at_ms: int


# Functional TypedDict supports the Rust wire key that is a Python keyword.
GraderReservation = TypedDict(
    "GraderReservation",
    {
        "cell": str,
        "attempt_sequence": int,
        "scorer": str,
        "scorer_version": int,
        "pass": int,
        "lock_digest": str,
        "request_digest": str,
        "started_at_ms": int,
    },
)


class GraderOutcome(TypedDict):
    """Independent grader classification and spend, retained even if scoring fails."""

    status: AttemptStatus
    reconciliation: Reconciliation
    failure_code: str | None
    locator: OperationLocator | None
    usage: MeasuredUsage
    completed_at_ms: int


class GraderRecord(TypedDict):
    """Persisted grader admission, execution and outcome."""

    reservation: GraderReservation
    execution: ExecutionIdentity | None
    outcome: GraderOutcome | None


class FrozenExperiment(TypedDict):
    """Immutable specification and engine provenance; includes private task bodies."""

    spec: dict[str, JsonValue]
    digest: str
    engine_version: str


class StoreSnapshot(TypedDict):
    """Copy of Rust store state; changes to it cannot mutate the store."""

    frozen: FrozenExperiment | None
    subject_locks: dict[str, str]
    reservations: dict[str, list[AttemptReservation]]
    attempts: dict[str, list[AttemptRecord]]
    graders: dict[str, GraderRecord]


class Statistics(TypedDict):
    """Derived decimal-string mean/stderr; n is the actual observation count."""

    n: int
    mean: str | None
    stderr: str | None


class MetricKey(TypedDict):
    """Scorer versions and score names are never mixed by aggregation."""

    scorer: str
    scorer_version: int
    name: str


class Aggregate(TypedDict):
    """Repetitions reduced within tasks, then mean/stderr across tasks."""

    subject: str
    metric: MetricKey
    statistics: Statistics
    expected_tasks: int
    complete_tasks: int
    scored_cells: int
    expected_cells: int


class OutcomeCounts(TypedDict):
    """Current classifications plus historical attempt counts."""

    expected: int
    completed: int
    subject_failed: int
    infrastructure_failed: int
    indeterminate: int
    pending: int
    unattempted: int
    attempts: int
    infrastructure_attempts: int
    scoring_failed: int
    scoring_pending: int
    graders_pending: int


class Spending(TypedDict):
    """Unit-separated exact decimal micros, including all historical graders."""

    subject_micros: dict[str, str]
    grader_micros: dict[str, str]
    total_micros: dict[str, str]
    subject_unknown: int
    grader_unknown: int


class PairedMetric(TypedDict):
    """Candidate-minus-baseline raw cell grades with unmatched coverage."""

    metric: MetricKey
    delta: Statistics
    wins: int
    ties: int
    losses: int
    baseline_only: int
    candidate_only: int
    missing_pairs: int


class PairedCost(TypedDict):
    """Same-unit matched cost differences; exact signed sum plus approximate stats."""

    unit: str
    total_delta_micros: str
    delta_micros: Statistics


class PairedComparison(TypedDict):
    """Quality and measurement pairs sharing task/repetition coordinates."""

    baseline: str
    candidate: str
    expected_pairs: int
    execution_pairs: int
    metrics: list[PairedMetric]
    input_token_delta: Statistics
    output_token_delta: Statistics
    total_token_delta: Statistics
    costs: list[PairedCost]
    missing_cost_pairs: int


class EvalReport(TypedDict):
    """Body-free Rust report, with first subject as the paired baseline."""

    spec_digest: str
    engine_version: str
    subject_locks: dict[str, str]
    counts: OutcomeCounts
    spending: Spending
    aggregates: list[Aggregate]
    comparisons: list[PairedComparison]


class GateResult(TypedDict):
    """Independent failure reasons; incomplete results cannot pass."""

    passed: bool
    quality_failed: bool
    infrastructure_failed: bool
    scoring_failed: bool
    incomplete: bool


class PreparedRequest(TypedDict, total=False):
    """Optional preparation changes; cannot replace the agent, journal or authority."""

    input: str
    settings: ModelSettings
    timeout_ms: int
    max_cycles: int


class PreparationContext(TypedDict):
    """Data-only preparation input; target answers are not exposed to subjects."""

    cell: Cell
    input: str
    settings: ModelSettings


class EvalOutput(TypedDict):
    """Final answer view for scoring; absent on failed subjects."""

    text: str
    structured_json: str | None


class ScoreContext(TypedDict):
    """Coarse trusted scorer input, bounded to one MiB at the callback boundary."""

    sample: TaskSampleData
    output: EvalOutput | None
    record: AttemptRecord
