"""Runtime, stub, and package-export parity for the Python binding."""

from __future__ import annotations

import ast
import inspect
from pathlib import Path
from types import ModuleType
from typing import Any

import finstack_ai
from finstack_ai import _finstack_ai as native

_TYPED_ONLY_NAMES = {
    "ActiveCapability",
    "Callback",
    "CapabilityActivation",
    "CapabilityCatalogItem",
    "ObserverCallback",
    "ObserverDiagnostic",
    "ObserverDiagnostics",
    "ParsedDocument",
    "RunStateSnapshot",
    "SessionInspectPhase",
    "SessionInspectSnapshot",
}
_REQUIRED = "<required>"


def _stub_tree() -> ast.Module:
    path = Path(finstack_ai.__file__).resolve().with_name("_finstack_ai.pyi")
    return ast.parse(path.read_text(encoding="utf-8"), filename=str(path))


def _top_level_names(tree: ast.Module) -> set[str]:
    names: set[str] = set()
    for node in tree.body:
        if isinstance(node, (ast.ClassDef, ast.FunctionDef, ast.AsyncFunctionDef)):
            names.add(node.name)
        elif isinstance(node, ast.AnnAssign) and isinstance(node.target, ast.Name):
            names.add(node.target.id)
        elif isinstance(node, ast.Assign):
            names.update(
                target.id for target in node.targets if isinstance(target, ast.Name)
            )
    return names


def _class_members(node: ast.ClassDef) -> set[str]:
    members: set[str] = set()
    for item in node.body:
        if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef)):
            members.add(item.name)
        elif isinstance(item, ast.AnnAssign) and isinstance(item.target, ast.Name):
            members.add(item.target.id)
        elif isinstance(item, ast.Assign):
            members.update(
                target.id for target in item.targets if isinstance(target, ast.Name)
            )
    return {name for name in members if not name.startswith("_")}


def _stub_parameters(
    node: ast.FunctionDef | ast.AsyncFunctionDef,
) -> list[tuple[str, str, str]]:
    positional = [*node.args.posonlyargs, *node.args.args]
    defaults: list[ast.expr | None] = [None] * (
        len(positional) - len(node.args.defaults)
    ) + list(node.args.defaults)
    parameters: list[tuple[str, str, str]] = []
    for argument, default in zip(positional, defaults, strict=True):
        if argument.arg in {"self", "cls"}:
            continue
        parameters.append(
            (
                argument.arg,
                "positional",
                _REQUIRED if default is None else ast.unparse(default),
            )
        )
    if node.args.vararg is not None:
        parameters.append((node.args.vararg.arg, "var_positional", _REQUIRED))
    for argument, default in zip(
        node.args.kwonlyargs, node.args.kw_defaults, strict=True
    ):
        parameters.append(
            (
                argument.arg,
                "keyword_only",
                _REQUIRED if default is None else ast.unparse(default),
            )
        )
    if node.args.kwarg is not None:
        parameters.append((node.args.kwarg.arg, "var_keyword", _REQUIRED))
    return parameters


def _runtime_default(value: Any) -> str:
    if value is inspect.Parameter.empty:
        return _REQUIRED
    if value is Ellipsis:
        return "..."
    return repr(value)


def _runtime_parameters(value: Any) -> list[tuple[str, str, str]] | None:
    try:
        signature = inspect.signature(value)
    except (TypeError, ValueError):
        return None
    kinds = {
        inspect.Parameter.POSITIONAL_ONLY: "positional",
        inspect.Parameter.POSITIONAL_OR_KEYWORD: "positional",
        inspect.Parameter.VAR_POSITIONAL: "var_positional",
        inspect.Parameter.KEYWORD_ONLY: "keyword_only",
        inspect.Parameter.VAR_KEYWORD: "var_keyword",
    }
    return [
        (parameter.name, kinds[parameter.kind], _runtime_default(parameter.default))
        for parameter in signature.parameters.values()
        if parameter.name not in {"self", "cls"}
    ]


def _runtime_public(module: ModuleType) -> set[str]:
    return {name for name in dir(module) if not name.startswith("_")}


def test_native_module_and_facade_exports_are_complete() -> None:
    tree = _stub_tree()
    stub_names = _top_level_names(tree)
    assert _runtime_public(native) <= stub_names
    assert all(
        hasattr(native, name)
        for name in stub_names - _TYPED_ONLY_NAMES
        if not name.startswith("_")
    )
    assert len(finstack_ai.__all__) == len(set(finstack_ai.__all__))
    assert all(hasattr(finstack_ai, name) for name in finstack_ai.__all__)
    discovered = {
        name
        for name in dir(finstack_ai)
        if not name.startswith("_") and name not in {"TypedDict", "cast", "providers"}
    }
    assert discovered == set(finstack_ai.__all__) - {"__version__"}


def test_native_class_members_match_the_stub() -> None:
    for node in (item for item in _stub_tree().body if isinstance(item, ast.ClassDef)):
        runtime_type = getattr(native, node.name, None)
        if not inspect.isclass(runtime_type):
            continue
        if issubclass(runtime_type, BaseException):
            continue
        stub_members = _class_members(node)
        runtime_members = {
            name for name in runtime_type.__dict__ if not name.startswith("_")
        }
        assert runtime_members == stub_members, node.name


def test_runtime_signatures_match_stub_names_kinds_and_defaults() -> None:
    tree = _stub_tree()
    for node in tree.body:
        if isinstance(node, (ast.FunctionDef, ast.AsyncFunctionDef)):
            runtime_value = getattr(native, node.name, None)
            if runtime_value is not None:
                assert _runtime_parameters(runtime_value) == _stub_parameters(node), (
                    node.name
                )
            continue
        if not isinstance(node, ast.ClassDef):
            continue
        runtime_type = getattr(native, node.name, None)
        if not inspect.isclass(runtime_type):
            continue
        for method in node.body:
            if not isinstance(method, (ast.FunctionDef, ast.AsyncFunctionDef)):
                continue
            if method.name == "__init__":
                runtime_value = runtime_type
            elif method.name.startswith("_"):
                continue
            else:
                runtime_value = getattr(runtime_type, method.name, None)
            if runtime_value is None or not callable(runtime_value):
                continue
            runtime_parameters = _runtime_parameters(runtime_value)
            if runtime_parameters is not None:
                assert runtime_parameters == _stub_parameters(method), (
                    f"{node.name}.{method.name}"
                )
