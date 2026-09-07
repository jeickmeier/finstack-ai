"""Typed normalized results for trusted Python model callbacks."""

from typing import NotRequired, TypeAlias, TypedDict

JsonValue: TypeAlias = (
    str | int | float | bool | None | list["JsonValue"] | dict[str, "JsonValue"]
)


class CostAmount(TypedDict):
    """Journal receipt cost in exact decimal micros under an accepted pricing policy."""

    unit: str
    micros: str
    pricing_policy_version: str


class Usage(TypedDict, total=False):
    """Canonical reported usage; omitted dimensions remain unknown, never zero."""

    input_tokens: int
    output_tokens: int
    total_tokens: int
    cost: CostAmount
    extension_counters: dict[str, int]


class ModelToolCall(TypedDict):
    """One normalized tool request from a coarse Python model callback."""

    name: str
    arguments: JsonValue
    provider_call_id: NotRequired[str | None]


class ModelOutput(TypedDict):
    """PythonModel callback result, validated and journaled by the Rust model adapter.

    Supply text or json, never both. Usage is optional; a reported cost must match
    the run's accepted pricing policy. Callback exceptions are safely classified.
    """

    completion_id: str
    text: NotRequired[str]
    json: NotRequired[JsonValue]
    tool_calls: NotRequired[list[ModelToolCall]]
    deferred: NotRequired[str | None]
    usage: NotRequired[Usage]


class ModelSettings(TypedDict):
    """Canonical provider settings wrapper passed through preparation callbacks."""

    values: JsonValue
