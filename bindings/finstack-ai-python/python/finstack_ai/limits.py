"""Typed kernel run limits accepted into a newly resolved Agent composition."""

from typing import Literal, TypedDict


class CostLimit(TypedDict):
    """Exact accepted pricing policy; monetary micros use a decimal string."""

    unit: str
    micros: str
    pricing_policy_version: str
    unknown_usage: Literal[
        "fail_closed", "suspend_for_decision", "allow_within_reserved_maximum"
    ]


class DurationData(TypedDict):
    """Kernel duration in seconds plus nanosecond remainder."""

    secs: int
    nanos: int


class RunLimits(TypedDict, total=False):
    """Optional ceilings accepted before dispatch; omitted fields have no ceiling.

    Apply with ``await agent.with_limits(limits)``. The returned agent has a new
    resolved lock; existing executions retain their original authority/limits.
    """

    max_model_requests: int
    max_turns: int
    max_tool_calls: int
    max_parallel_tools: int
    max_input_tokens: int
    max_output_tokens: int
    max_context_bytes: int
    max_output_bytes: int
    max_retries: int
    max_wall_time: DurationData
    max_cost: CostLimit
    extension_counters: dict[str, int]
