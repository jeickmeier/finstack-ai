"""Typed data shapes and the Rust-owned embedded durable host."""

from typing import Any, NotRequired, TypedDict

from ._finstack_ai import DurableHost


class DurableTickReport(TypedDict):
    """Worker counters; failures retain durable work for reconciliation."""

    cron_fires: int
    runs_started: int
    sessions_resumed: int
    sessions_reparked: int
    sessions_expired: int
    responses_rejected: int
    failures: int


class DurableInspection(TypedDict):
    """Committed state and optional completed message for one original run."""

    locator: dict[str, str]
    workflow_kind: str
    phase: str | None
    terminal: bool
    state: dict[str, Any]
    message: dict[str, Any] | None


class DurableInteraction(TypedDict):
    """Pending interaction, including its exact accepted authorization context."""

    interaction_id: str
    tenant_scope: str
    session_id: str
    lane_id: str
    run_id: str
    request: dict[str, Any]
    principal: dict[str, Any]
    evidence: dict[str, Any]
    status: str


class ResolutionInput(TypedDict):
    """Application-authorized response; Rust checks principal and evidence."""

    resolution_id: str
    principal: dict[str, Any]
    evidence: dict[str, Any]
    payload: dict[str, Any]
    note: NotRequired[str | None]


__all__ = [
    "DurableHost",
    "DurableInspection",
    "DurableInteraction",
    "DurableTickReport",
    "ResolutionInput",
]
