"""Async Rust-owned evaluation operations and immutable report handles."""

from __future__ import annotations

from collections.abc import Awaitable, Callable, Sequence
from os import PathLike, fspath
from typing import TypeAlias, cast

from .._finstack_ai import Agent, _EvalResult, _EvalRunner, _EvalStore, _EvalSubject
from ._types import (
    EvalReport,
    GateResult,
    PreparationContext,
    PreparedRequest,
    StoreSnapshot,
)
from .config import EvalSpec, ThresholdGate
from .scorers import Scorer

SubjectCallback: TypeAlias = Callable[
    [PreparationContext], PreparedRequest | Awaitable[PreparedRequest]
]


class SubjectBinding:
    """Bind an existing Agent to a declared arm; every cell gets a real fresh session.

    Credentials, stores, tools and resolved configuration remain on the Agent.
    Preparation callbacks can change bounded input/settings/deadline/cycle
    limits. They cannot replace the bound agent, journal, security or attachments.
    Preparation must not dispatch work. Rust owns reservation and admission.
    """

    def __init__(
        self,
        subject_id: str,
        agent: Agent,
        *,
        tenant_scope: str = "python-local",
        prepare: SubjectCallback | None = None,
        callback_timeout_seconds: float = 30.0,
    ) -> None:
        self._native = _EvalSubject(
            subject_id,
            agent,
            tenant_scope=tenant_scope,
            prepare=prepare,
            callback_timeout_seconds=callback_timeout_seconds,
        )


class EvalStore:
    """Store handle; use MemoryEvalStore or SqliteEvalStore to construct one."""

    def __init__(self, native: _EvalStore) -> None:
        self._native = native

    def snapshot(self) -> EvalRunReport:
        """Project acknowledged state without running subjects or scorers.

        Raises:
            FinstackError: Store is unavailable or has no frozen experiment.
        """
        return EvalRunReport(self._native.snapshot())


class MemoryEvalStore(EvalStore):
    """Bounded in-process append-only experiment store with one runner owner."""

    def __init__(self) -> None:
        super().__init__(_EvalStore.memory())


class SqliteEvalStore(EvalStore):
    """Durable append-only SQLite experiment storage in a dedicated database file.

    Uses schema version 1, WAL and FULL durability, with a cross-process runner
    lease. Do not share this file with the application journal or durable host.
    """

    def __init__(self, path: str | PathLike[str]) -> None:
        super().__init__(_EvalStore.sqlite(fspath(path)))


class EvalRunReport:
    """Immutable native snapshot and body-free report; Python properties return copies."""

    def __init__(self, native: _EvalResult) -> None:
        self._native = native

    @property
    def stop_reason(self) -> str | None:
        """Stable reason admission stopped, or None after eligible work finished."""
        return self._native.stop_reason

    @property
    def report(self) -> EvalReport:
        """Rust aggregates, paired comparisons, spending and explicit failure coverage."""
        return cast(EvalReport, self._native.report())

    @property
    def snapshot(self) -> StoreSnapshot:
        """Full store copy, including private task bodies; use exports for sharing."""
        return cast(StoreSnapshot, self._native.snapshot())

    def evaluate(self, gate: ThresholdGate) -> GateResult:
        """Evaluate thresholds and failure coverage in Rust.

        Raises:
            FinstackError: Invalid gate (no absolute or baseline threshold).
        """
        return cast(GateResult, self._native.gate(gate._json()))

    def export_jsonl(self, path: str | PathLike[str]) -> None:
        """Write authoritative attempt rows, excluding input/target/metadata bodies.

        Integer measurements and costs are decimal strings. File I/O runs without
        the GIL. The path is created or replaced; failures raise FinstackError.
        """
        self._native.export_jsonl(fspath(path))

    def write_summary(self, path: str | PathLike[str]) -> None:
        """Write body-free summary.json with decimal-string statistics and exact cost.

        The path is created or replaced; writer failures raise FinstackError.
        """
        self._native.write_summary(fspath(path))


class EvalRunner:
    """Run, resume and rescore an experiment using the Rust evaluation engine.

    Args:
        spec: Immutable experiment inputs and bounds.
        store: Memory or dedicated SQLite experiment store.
        subjects: One existing-agent binding per declared arm.
        scorers: One versioned implementation per declared scorer.

    Raises:
        FinstackError: Invalid configuration or mismatched bindings. Freeze/lock
            checks and storage failures may also arise when running/resuming.
    """

    def __init__(
        self,
        spec: EvalSpec,
        store: EvalStore,
        subjects: Sequence[SubjectBinding],
        scorers: Sequence[Scorer],
    ) -> None:
        self._native = _EvalRunner(
            spec._json(),
            store._native,
            [s._native for s in subjects],
            [s._native for s in scorers],
        )

    def cancel(self) -> None:
        """Stop admission and cancel active work; await the operation for settlement.

        A cancelled instance stays cancelled. Construct another runner to resume.
        Cancelling/dropping a Python await alone does not cancel the native owner.
        """
        self._native.cancel()

    async def run(self) -> EvalRunReport:
        """Freeze, reconcile admitted work, run eligible cells and grade final results.

        Rust retains the store lease through cancellation/settlement. An unresolved
        attempt is never silently repeated. FinstackError reports preflight/store
        errors; the result's stop_reason describes admission stops.
        """
        return EvalRunReport(await self._native.run())

    async def resume(self) -> EvalRunReport:
        """Reconcile then skip final subjects and all existing scoring passes."""
        return EvalRunReport(await self._native.resume())

    async def rescore(self) -> EvalRunReport:
        """Append new passes from journals without subject preparation/model calls.

        Judge scorers may run new graders and incur separately recorded spending.
        Missing journals become scoring failures; unresolved graders block repeats.
        """
        return EvalRunReport(await self._native.rescore())
