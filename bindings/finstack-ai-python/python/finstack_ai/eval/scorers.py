"""Built-in and custom scorer bindings over the canonical Rust implementations."""

from __future__ import annotations

import json
from collections.abc import Awaitable, Callable, Mapping, Sequence
from dataclasses import asdict, dataclass, field
from typing import Literal, TypeAlias

from .._finstack_ai import Agent, _EvalScorer
from ._types import Score, ScoreContext
from .config import _encode

ScorerCallback: TypeAlias = Callable[
    [ScoreContext], Sequence[Score] | Awaitable[Sequence[Score]]
]


class Scorer:
    """Versioned scorer handle; construct a built-in scorer or PythonScorer."""

    def __init__(self, native: _EvalScorer) -> None:
        self._native = native


class ExactMatchScorer(Scorer):
    """Compare the whole answer with the target using explicit normalization."""

    def __init__(
        self,
        scorer_id: str = "exact_match",
        version: int = 1,
        *,
        trim: bool = True,
        case_sensitive: bool = True,
    ) -> None:
        """Bind equality behavior; Rust rejects invalid identities/versions."""
        super().__init__(
            _EvalScorer.builtin(
                scorer_id,
                version,
                _encode(
                    {
                        "kind": "exact_match",
                        "trim": trim,
                        "case_sensitive": case_sensitive,
                    }
                ),
            )
        )


class IncludesScorer(Scorer):
    """Find a nonempty literal target inside the answer; no regex interpretation."""

    def __init__(
        self,
        scorer_id: str = "includes",
        version: int = 1,
        *,
        case_sensitive: bool = True,
    ) -> None:
        super().__init__(
            _EvalScorer.builtin(
                scorer_id,
                version,
                _encode({"kind": "includes", "case_sensitive": case_sensitive}),
            )
        )


class RegexScorer(Scorer):
    """Use the bounded, precompiled Rust regex engine on the final answer."""

    def __init__(
        self, pattern: str, scorer_id: str = "regex", version: int = 1
    ) -> None:
        super().__init__(
            _EvalScorer.builtin(
                scorer_id, version, _encode({"kind": "regex", "pattern": pattern})
            )
        )


@dataclass(frozen=True)
class ToleranceBands:
    """Exact decimal full/partial relative bands and optional absolute tolerance.

    Relative bands are integer parts per million. Absolute is parsed by Rust
    (for example ``"USD 10"``). Partial credit uses integer score micros.
    """

    full_within_ppm: int = 0
    partial_within_ppm: int | None = None
    partial_micros: int = 0
    absolute: str | None = None

    def _wire(self) -> dict[str, object]:
        return {
            "full_within_ppm": self.full_within_ppm,
            "partial_within_ppm": self.partial_within_ppm,
            "partial_micros": self.partial_micros,
            "absolute": None
            if self.absolute is None
            else json.loads(_EvalScorer.parse_number(self.absolute)),
        }


class NumericToleranceScorer(Scorer):
    """Grade exact decimal values with unit-aware tolerance arithmetic in Rust."""

    def __init__(
        self,
        bands: ToleranceBands,
        scorer_id: str = "numeric_tolerance",
        version: int = 1,
    ) -> None:
        super().__init__(
            _EvalScorer.builtin(
                scorer_id,
                version,
                _encode({"kind": "numeric_tolerance", "bands": bands._wire()}),
            )
        )


@dataclass(frozen=True)
class FieldTolerance:
    """Exact JSON equality or relative numeric tolerance in parts per million."""

    kind: Literal["exact", "numeric_ppm"] = "exact"
    ppm: int | None = None

    def _wire(self) -> dict[str, object]:
        if self.kind == "exact":
            return {"kind": self.kind}
        return {"kind": self.kind, "ppm": self.ppm}


@dataclass(frozen=True)
class FieldSpec:
    """Weighted RFC 6901 field; missing answer fields receive zero credit."""

    pointer: str
    tolerance: FieldTolerance = field(default_factory=FieldTolerance)
    weight: int = 1

    def _wire(self) -> dict[str, object]:
        return {
            "pointer": self.pointer,
            "tolerance": self.tolerance._wire(),
            "weight": self.weight,
        }


class StructuredFieldScorer(Scorer):
    """Emit one grade per selected JSON field and an integer weighted aggregate."""

    def __init__(
        self,
        fields: Sequence[FieldSpec],
        scorer_id: str = "structured_field",
        version: int = 1,
    ) -> None:
        super().__init__(
            _EvalScorer.builtin(
                scorer_id,
                version,
                _encode(
                    {
                        "kind": "structured_field",
                        "fields": [value._wire() for value in fields],
                    }
                ),
            )
        )


class RecordKindsScorer(Scorer):
    """Check required/forbidden committed record kinds with complete history."""

    def __init__(
        self,
        scorer_id: str = "record_kinds",
        version: int = 1,
        *,
        required: Sequence[str] = (),
        forbidden: Sequence[str] = (),
    ) -> None:
        super().__init__(
            _EvalScorer.builtin(
                scorer_id,
                version,
                _encode(
                    {
                        "kind": "record_kinds",
                        "required": required,
                        "forbidden": forbidden,
                    }
                ),
            )
        )


@dataclass(frozen=True)
class JudgeRubric:
    """Host instructions and allowed labels mapped to exact score micros.

    Configured tools/capabilities are rejected unless allow_tools is explicit.
    Inputs are untrusted evidence; model-authored explanations are not persisted.
    """

    instruction: str
    choices: Mapping[str, int]
    pass_threshold_micros: int = 1_000_000
    allow_tools: bool = False


class JudgeScorer(Scorer):
    """Constrained Rust agent grader with its own durable admission and accounting."""

    def __init__(
        self,
        agent: Agent,
        rubric: JudgeRubric,
        scorer_id: str = "judge",
        version: int = 1,
        *,
        tenant_scope: str = "python-local",
    ) -> None:
        """Bind the grader without dispatching; invalid rubrics/tools raise FinstackError."""
        super().__init__(
            _EvalScorer.judge(
                scorer_id,
                version,
                agent,
                _encode(asdict(rubric)),
                tenant_scope=tenant_scope,
            )
        )


class PythonScorer(Scorer):
    """Trusted coarse scorer callback; Rust validates and persists returned scores.

    The callback receives optional failed-subject output and must return a list
    of Score values with this scorer's identity/version. Async callbacks receive
    cancellation on timeout or runner cancellation. Synchronous callbacks run in
    the blocking pool: their late result is ignored, but Python threads cannot
    be forcibly stopped. Use async callbacks for cooperative interruption.
    """

    def __init__(
        self,
        scorer_id: str,
        version: int,
        callback: ScorerCallback,
        *,
        callback_timeout_seconds: float = 30.0,
    ) -> None:
        super().__init__(
            _EvalScorer.python(
                scorer_id,
                version,
                callback,
                callback_timeout_seconds=callback_timeout_seconds,
            )
        )
