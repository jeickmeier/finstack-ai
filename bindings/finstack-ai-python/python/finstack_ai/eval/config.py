"""Typed immutable configuration; Rust performs validation and canonical hashing."""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import asdict, dataclass, field
from typing import Literal

from .._finstack_ai import _eval_spec_digest
from ._types import ArtifactReference, JsonValue, MetricKey


def _encode(value: object) -> str:
    return json.dumps(value, ensure_ascii=False, allow_nan=False, separators=(",", ":"))


@dataclass(frozen=True)
class TaskSample:
    """Dataset input/target with optional scoped artifact references.

    Rust validates sizes and identities when constructing the runner. Input and
    target remain outside result exports. Stage attachments with the same durable
    artifact store supplied to the bound Agent.
    """

    task_id: str
    input: str
    target: str
    metadata: JsonValue = None
    attachments: Sequence[ArtifactReference] = ()


@dataclass(frozen=True)
class SubjectDecl:
    """Stable subject identity and optional expected resolved-lock fingerprint."""

    subject_id: str
    lock_digest: str | None = None


@dataclass(frozen=True)
class RepetitionReducer:
    """Reduce repetitions within each task before reporting across-task statistics.

    Use ``mean``, ``pass_at_k`` or ``at_least_k``. k must be positive and no larger
    than the experiment's repetitions. Rust owns the arithmetic and validation.
    """

    kind: Literal["mean", "pass_at_k", "at_least_k"] = "mean"
    k: int | None = None
    pass_threshold_micros: int = 1_000_000

    def _wire(self) -> dict[str, object]:
        if self.kind == "mean":
            return {"kind": self.kind}
        return {
            "kind": self.kind,
            "k": self.k,
            "pass_threshold_micros": self.pass_threshold_micros,
        }


@dataclass(frozen=True)
class EvalLimits:
    """Bound admission and execution; monetary budgets are admission thresholds.

    Finite budgets require compatible accepted pricing policies. Concurrent
    in-flight execution can exceed the threshold; unknown cost stops admission.
    """

    attempt_timeout_ms: int = 120_000
    max_replacement_attempts: int = 2
    max_concurrency: int = 4
    budget_micros: int | None = None
    budget_unit: str | None = None


@dataclass(frozen=True)
class EvalSpec:
    """Frozen task × repetition × subject experiment, validated by Rust.

    Args:
        name: Bounded display name.
        tasks: Dataset in stable task order.
        subjects: Arms in stable order; the first is the reporting baseline.
        scorers: Unique scorer identifiers bound to implementations by EvalRunner.
        repetitions: Independent executions per task and subject.
        reducer: Task-level repetition reduction.
        limits: Scheduling, replacement, deadline and monetary bounds.

    Raises:
        FinstackError: Rust rejects invalid or divergent specs before execution.
    """

    name: str
    tasks: Sequence[TaskSample]
    subjects: Sequence[SubjectDecl]
    scorers: Sequence[str]
    repetitions: int = 1
    reducer: RepetitionReducer = field(default_factory=RepetitionReducer)
    limits: EvalLimits = field(default_factory=EvalLimits)
    schema_version: str = "finstack.eval.v1"

    def _json(self) -> str:
        value = asdict(self)
        value["reducer"] = self.reducer._wire()
        return _encode(value)

    def digest(self) -> str:
        """Validate in Rust and return the canonical credential-free spec digest.

        Raises:
            FinstackError: Invalid spec identity, bounds or configuration.
        """
        return _eval_spec_digest(self._json())


@dataclass(frozen=True)
class ThresholdGate:
    """Rust gate for one exact metric and experiment-wide execution/scoring health.

    Minimum mean applies after repetition reduction. Maximum regression applies
    to paired raw cell deltas against the first declared subject. Missing scores
    or metric versions cannot pass. Use ``EvalRunReport.evaluate(gate)``.
    """

    subject: str
    metric: MetricKey
    minimum_mean: int | None = None
    maximum_regression: int | None = None

    def _json(self) -> str:
        return _encode(asdict(self))
