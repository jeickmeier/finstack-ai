"""Optional Pydantic ergonomics over the Rust-owned Toolset port."""

from __future__ import annotations

import functools
import importlib
import inspect
import json
from collections.abc import Callable
from typing import Any, TypeVar, cast, overload

from . import _finstack_ai as _native


Function = TypeVar("Function", bound=Callable[..., Any])


def _pydantic() -> Any:
    try:
        return importlib.import_module("pydantic")
    except ModuleNotFoundError as error:
        if error.name == "pydantic":
            raise TypeError(
                "Pydantic adapters require the optional extra: "
                "install finstack-ai[pydantic]"
            ) from error
        raise


def _type_adapter(target: Any) -> Any:
    adapter_type = _pydantic().TypeAdapter
    return target if isinstance(target, adapter_type) else adapter_type(target)


def _normalized_schema(adapter: Any, *, mode: str, kind: str) -> dict[str, Any]:
    schema = adapter.json_schema(mode=mode)
    normalized = _native._normalize_pydantic_schema(schema, kind)
    if not isinstance(normalized, dict):
        raise TypeError("normalized Pydantic schema must be an object")
    return normalized


class PydanticTool:
    """One cached annotated Python tool registration.

    The decorated callable remains directly callable. Use
    :func:`pydantic_toolset` to publish it through the Rust Toolset port.
    """

    def __init__(
        self,
        function: Callable[..., Any],
        *,
        name: str | None = None,
        title: str | None = None,
        description: str | None = None,
        component_id: str | None = None,
        side_effect: str = "read_only",
        retry_safety: str = "safe_to_retry",
        max_result_bytes: int = 1_048_576,
    ) -> None:
        if not callable(function):
            raise TypeError("@tool target must be callable")
        self._function = function
        self.name = name or function.__name__
        self.title = title or self.name.replace("_", " ").title()
        self.description = (
            description or inspect.getdoc(function) or f"Python tool {self.name}."
        )
        self.component_id = component_id or f"python.{self.name}"
        self.side_effect = side_effect
        self.retry_safety = retry_safety
        self.max_result_bytes = max_result_bytes
        self._input_adapter: Any = None
        self._output_adapter: Any = None
        self._input_schema: dict[str, Any] = {}
        self._output_schema: dict[str, Any] | None = None
        self._schema_generation = 0
        functools.update_wrapper(self, function)
        self.refresh_schema()

    def __call__(self, *args: Any, **kwargs: Any) -> Any:
        """Call the original Python function directly."""

        return self._function(*args, **kwargs)

    @property
    def input_schema(self) -> dict[str, Any]:
        """Return a detached copy of the cached input schema."""

        return cast(dict[str, Any], json.loads(json.dumps(self._input_schema)))

    @property
    def output_schema(self) -> dict[str, Any] | None:
        """Return a detached copy of the cached output schema."""

        if self._output_schema is None:
            return None
        return cast(dict[str, Any], json.loads(json.dumps(self._output_schema)))

    @property
    def schema_generation(self) -> int:
        """Number of explicit schema-generation passes for this tool."""

        return self._schema_generation

    def refresh_schema(self) -> None:
        """Regenerate adapters and schemas before a new Toolset registration."""

        signature = inspect.signature(self._function)
        hints = inspect.get_annotations(self._function, eval_str=True)
        fields: dict[str, Any] = {}
        for parameter in signature.parameters.values():
            if parameter.kind in {
                inspect.Parameter.POSITIONAL_ONLY,
                inspect.Parameter.VAR_POSITIONAL,
                inspect.Parameter.VAR_KEYWORD,
            }:
                raise TypeError(
                    f"unsupported parameter kind for '{parameter.name}' in @tool {self.name}"
                )
            if parameter.annotation is inspect.Parameter.empty:
                raise TypeError(
                    f"parameter '{parameter.name}' in @tool {self.name} requires an annotation"
                )
            if parameter.default is not inspect.Parameter.empty:
                raise TypeError(
                    f"parameter '{parameter.name}' in @tool {self.name} has a default; "
                    "the portable provider subset requires every argument"
                )
            fields[parameter.name] = hints.get(parameter.name, parameter.annotation)

        extensions = importlib.import_module("typing_extensions")
        input_type = extensions.TypedDict(f"{self.name.title()}Arguments", fields)
        self._input_adapter = _type_adapter(input_type)
        self._input_schema = _normalized_schema(
            self._input_adapter,
            mode="validation",
            kind="tool_input",
        )

        return_type = hints.get("return", signature.return_annotation)
        if return_type is inspect.Signature.empty:
            self._output_adapter = None
            self._output_schema = None
        else:
            self._output_adapter = _type_adapter(return_type)
            self._output_schema = _normalized_schema(
                self._output_adapter,
                mode="serialization",
                kind="tool_output",
            )
        self._schema_generation += 1

    def _spec(self) -> dict[str, Any]:
        requirement = "not_required" if self.side_effect == "read_only" else "required"
        return {
            "id": self.component_id,
            "model_name": self.name,
            "title": self.title,
            "description": self.description,
            "input_schema": self.input_schema,
            "output_schema": self.output_schema,
            "execution": "sequential",
            "side_effect": self.side_effect,
            "retry_safety": self.retry_safety,
            "approval": {
                "requirement": requirement,
                "reason": None,
                "attributes": {},
            },
            "max_result_bytes": self.max_result_bytes,
            "metadata": {},
        }

    async def _invoke(self, arguments: object) -> object:
        encoded = json.dumps(arguments, separators=(",", ":")).encode()
        values = self._input_adapter.validate_json(encoded)
        if not isinstance(values, dict):
            raise TypeError("annotated tool arguments must validate to a mapping")
        result = self._function(**values)
        if inspect.isawaitable(result):
            result = await result
        if self._output_adapter is None:
            return result
        try:
            validated = self._output_adapter.validate_python(result)
            return json.loads(self._output_adapter.dump_json(validated))
        except Exception:  # noqa: BLE001 - Rust validator owns the stable failure path.
            return None


@overload
def tool(function: Function, /) -> PydanticTool: ...


@overload
def tool(
    function: None = None,
    /,
    *,
    name: str | None = None,
    title: str | None = None,
    description: str | None = None,
    component_id: str | None = None,
    side_effect: str = "read_only",
    retry_safety: str = "safe_to_retry",
    max_result_bytes: int = 1_048_576,
) -> Callable[[Function], PydanticTool]: ...


def tool(
    function: Function | None = None,
    /,
    *,
    name: str | None = None,
    title: str | None = None,
    description: str | None = None,
    component_id: str | None = None,
    side_effect: str = "read_only",
    retry_safety: str = "safe_to_retry",
    max_result_bytes: int = 1_048_576,
) -> PydanticTool | Callable[[Function], PydanticTool]:
    """Decorate one fully annotated sync or async Python tool."""

    def decorate(target: Function) -> PydanticTool:
        return PydanticTool(
            target,
            name=name,
            title=title,
            description=description,
            component_id=component_id,
            side_effect=side_effect,
            retry_safety=retry_safety,
            max_result_bytes=max_result_bytes,
        )

    return decorate(function) if function is not None else decorate


def pydantic_toolset(
    *tools: PydanticTool,
    component: str,
    name: str,
    callback_timeout_seconds: float = 30.0,
) -> _native.PythonToolset:
    """Register cached decorated tools through the Rust Toolset port."""

    if not tools:
        raise TypeError("pydantic_toolset requires at least one @tool")
    by_name: dict[str, PydanticTool] = {}
    for registered in tools:
        if not isinstance(registered, PydanticTool):
            raise TypeError("pydantic_toolset accepts only @tool values")
        if registered.name in by_name:
            raise TypeError(f"duplicate Pydantic tool name: {registered.name}")
        by_name[registered.name] = registered

    async def callback(
        context: _native.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context
        call = request.get("call")
        if not isinstance(call, dict):
            raise TypeError("validated tool call is missing")
        tool_name = call.get("tool_name")
        arguments = call.get("arguments")
        if not isinstance(tool_name, str) or tool_name not in by_name:
            raise TypeError("validated tool name is not registered")
        output = await by_name[tool_name]._invoke(arguments)
        return {"output": output, "is_error": False}

    return _native.PythonToolset(
        callback,
        component=component,
        name=name,
        tools=[registered._spec() for registered in tools],
        callback_timeout_seconds=callback_timeout_seconds,
    )


__all__ = ["PydanticTool", "pydantic_toolset", "tool"]
