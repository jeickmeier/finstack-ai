#!/usr/bin/env python3
"""Generate checked-in Rust bindings from the locked v0.0.4 WIT packages."""

from __future__ import annotations

import argparse
import re
import subprocess
import sys
import tempfile
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
WIT_ROOT = REPO_ROOT / "plugins" / "finstack-ai-wit" / "wit" / "v0.0.4"
GENERATED_RS = REPO_ROOT / "plugins" / "finstack-ai-wit" / "src" / "generated.rs"
CRATE_TOML = REPO_ROOT / "plugins" / "finstack-ai-wit" / "Cargo.toml"
CONTEXT_WIT = WIT_ROOT / "finstack-ai-context" / "context.wit"
ALLOWED_PACKAGES = (
    "finstack:ai-types@0.0.4",
    "finstack:ai-host@0.0.4",
    "finstack:ai-toolset@0.0.4",
    "finstack:ai-context@0.0.4",
)
FORBIDDEN_WORLD_TOKENS = (
    "agent",
    "session",
    "run",
    "lineage",
    "nested",
)
V1_PACKAGE = re.compile(r"package\s+[^;]+@1\.0\.0\b")
PACKAGE_RE = re.compile(r"^package\s+([^;]+);", re.MULTILINE)
IDENT_RE = re.compile(r"[A-Za-z][A-Za-z0-9-]*")


@dataclass
class Field:
    name: str
    ty: str


@dataclass
class Func:
    name: str
    params: list[Field]
    result: str


@dataclass
class Interface:
    name: str
    records: dict[str, list[Field]] = field(default_factory=dict)
    enums: dict[str, list[str]] = field(default_factory=dict)
    funcs: list[Func] = field(default_factory=list)


@dataclass
class World:
    name: str
    imports: list[str] = field(default_factory=list)
    exports: list[str] = field(default_factory=list)


@dataclass
class Package:
    name: str
    path: Path
    interfaces: list[Interface] = field(default_factory=list)
    worlds: list[World] = field(default_factory=list)


class WitError(RuntimeError):
    """WIT parse or policy failure."""


def strip_comments(text: str) -> str:
    return re.sub(r"//[^\n]*", "", text)


def kebab_to_pascal(name: str) -> str:
    return "".join(part[:1].upper() + part[1:] for part in name.split("-"))


def kebab_to_snake(name: str) -> str:
    return name.replace("-", "_")


def rust_type(ty: str) -> str:
    ty = ty.strip()
    if ty == "string":
        return "String"
    if ty == "bool":
        return "bool"
    if ty == "u32":
        return "u32"
    if ty == "u64":
        return "u64"
    if ty == "s64":
        return "i64"
    if ty == "_":
        return "()"
    option = re.fullmatch(r"option<(.+)>", ty)
    if option:
        return f"Option<{rust_type(option.group(1))}>"
    listed = re.fullmatch(r"list<(.+)>", ty)
    if listed:
        inner = listed.group(1).strip()
        if inner == "u8":
            return "Vec<u8>"
        return f"Vec<{rust_type(inner)}>"
    result = re.fullmatch(r"result<(.+),\s*(.+)>", ty)
    if result:
        return f"Result<{rust_type(result.group(1))}, {rust_type(result.group(2))}>"
    if IDENT_RE.fullmatch(ty):
        return kebab_to_pascal(ty)
    raise WitError(f"unsupported WIT type: {ty}")


def rust_param_type(ty: str) -> str:
    mapped = rust_type(ty)
    if mapped == "String":
        return "&str"
    if mapped == "Vec<u8>":
        return "&[u8]"
    if mapped.startswith("Vec<") or mapped[0].isupper():
        return f"&{mapped}"
    return mapped


def split_top_level(text: str, delimiter: str) -> list[str]:
    parts: list[str] = []
    depth = 0
    start = 0
    for index, char in enumerate(text):
        if char == "<":
            depth += 1
        elif char == ">":
            depth -= 1
        elif char == delimiter and depth == 0:
            parts.append(text[start:index])
            start = index + 1
    parts.append(text[start:])
    return parts


def parse_fields(body: str) -> list[Field]:
    fields: list[Field] = []
    for raw in split_top_level(body, ","):
        item = raw.strip()
        if not item:
            continue
        name, ty = item.split(":", 1)
        fields.append(Field(name=name.strip(), ty=ty.strip()))
    return fields


def parse_func(text: str) -> Func:
    match = re.fullmatch(
        r"([A-Za-z][A-Za-z0-9-]*)\s*:\s*func\s*\((.*)\)\s*(?:->\s*(.+))?",
        text,
        flags=re.DOTALL,
    )
    if match is None:
        raise WitError(f"unsupported func: {text}")
    params = parse_fields(match.group(2))
    result = match.group(3).strip() if match.group(3) else "()"
    return Func(name=match.group(1), params=params, result=result)


def parse_interface_body(body: str) -> Interface:
    interface = Interface(name="")
    index = 0
    length = len(body)
    while index < length:
        while index < length and body[index].isspace():
            index += 1
        if index >= length:
            break
        if body.startswith("use ", index):
            end = body.find(";", index)
            if end < 0:
                raise WitError("unterminated use")
            index = end + 1
            continue
        if body.startswith("record ", index):
            name_start = index + len("record ")
            name_end = body.find("{", name_start)
            name = body[name_start:name_end].strip()
            close = matching_brace(body, name_end)
            interface.records[name] = parse_fields(body[name_end + 1 : close])
            index = close + 1
            continue
        if body.startswith("enum ", index):
            name_start = index + len("enum ")
            name_end = body.find("{", name_start)
            name = body[name_start:name_end].strip()
            close = matching_brace(body, name_end)
            cases = [
                case.strip()
                for case in body[name_end + 1 : close].split(",")
                if case.strip()
            ]
            interface.enums[name] = cases
            index = close + 1
            continue
        end = body.find(";", index)
        if end < 0:
            raise WitError(f"unterminated interface item: {body[index:]}")
        interface.funcs.append(parse_func(re.sub(r"\s+", " ", body[index:end]).strip()))
        index = end + 1
    return interface


def matching_brace(text: str, open_index: int) -> int:
    depth = 0
    for index in range(open_index, len(text)):
        if text[index] == "{":
            depth += 1
        elif text[index] == "}":
            depth -= 1
            if depth == 0:
                return index
    raise WitError("unbalanced braces")


def parse_world_body(body: str) -> World:
    world = World(name="")
    for raw in body.split(";"):
        item = re.sub(r"\s+", " ", raw).strip()
        if not item:
            continue
        if item.startswith("import "):
            world.imports.append(item[len("import ") :].strip())
        elif item.startswith("export "):
            world.exports.append(item[len("export ") :].strip())
        else:
            raise WitError(f"unsupported world item: {item}")
    return world


def parse_package(path: Path) -> Package:
    text = strip_comments(path.read_text(encoding="utf-8"))
    if V1_PACKAGE.search(text):
        raise WitError(
            f"{path}: @1.0.0 package generation is blocked until framework 1.0"
        )
    package_match = PACKAGE_RE.search(text)
    if package_match is None:
        raise WitError(f"{path}: missing package declaration")
    package_name = package_match.group(1).strip()
    if package_name not in ALLOWED_PACKAGES:
        raise WitError(f"{path}: unexpected package {package_name}")
    if package_name.endswith("@1.0.0"):
        raise WitError(
            f"{path}: @1.0.0 package generation is blocked until framework 1.0"
        )
    package = Package(name=package_name, path=path)
    index = package_match.end()
    while index < len(text):
        while index < len(text) and text[index].isspace():
            index += 1
        if index >= len(text):
            break
        if text.startswith("interface ", index):
            name_start = index + len("interface ")
            name_end = text.find("{", name_start)
            name = text[name_start:name_end].strip()
            close = matching_brace(text, name_end)
            interface = parse_interface_body(text[name_end + 1 : close])
            interface.name = name
            package.interfaces.append(interface)
            index = close + 1
            continue
        if text.startswith("world ", index):
            name_start = index + len("world ")
            name_end = text.find("{", name_start)
            name = text[name_start:name_end].strip()
            close = matching_brace(text, name_end)
            world = parse_world_body(text[name_end + 1 : close])
            world.name = name
            lowered = " ".join([world.name, *world.exports, *world.imports]).lower()
            for token in forbidden_tokens_for_world(world.name):
                if token in lowered.split() or token in world.exports:
                    raise WitError(f"{path}: forbidden world surface token {token}")
            package.worlds.append(world)
            index = close + 1
            continue
        raise WitError(f"{path}: unexpected item at {text[index : index + 32]!r}")
    return package


def forbidden_tokens_for_world(world_name: str) -> tuple[str, ...]:
    extra = {
        "toolset-plugin": ("context-provider",),
        "context-plugin": ("toolset",),
    }
    return FORBIDDEN_WORLD_TOKENS + extra.get(world_name, ("context-provider", "toolset"))


def crate_version() -> str:
    match = re.search(
        r'^version(?:\.workspace)?\s*=\s*"([^"]+)"', CRATE_TOML.read_text(), re.M
    )
    if match is None:
        workspace = (REPO_ROOT / "Cargo.toml").read_text(encoding="utf-8")
        match = re.search(r'^version\s*=\s*"([^"]+)"', workspace, re.M)
    if match is None:
        raise WitError("unable to read crate version")
    version = match.group(1)
    if version == "1.0.0":
        raise WitError("plugin crate version 1.0.0 is blocked until framework 1.0")
    return version


def emit_record(name: str, fields: list[Field]) -> str:
    rust_name = kebab_to_pascal(name)
    lines = [
        f"/// WIT `{name}` record generated from the checked-in v0.0.4 packages.",
        "#[derive(Clone, Debug, PartialEq, Eq)]",
        f"pub struct {rust_name} {{",
    ]
    for member in fields:
        lines.append(f"    /// WIT field `{member.name}`.")
        lines.append(f"    pub {kebab_to_snake(member.name)}: {rust_type(member.ty)},")
    lines.append("}")
    return "\n".join(lines)


def emit_enum(name: str, cases: list[str]) -> str:
    rust_name = kebab_to_pascal(name)
    lines = [
        f"/// WIT `{name}` enum generated from the checked-in v0.0.4 packages.",
        "#[derive(Clone, Copy, Debug, PartialEq, Eq)]",
        f"pub enum {rust_name} {{",
    ]
    for case in cases:
        lines.append(f"    /// WIT case `{case}`.")
        lines.append(f"    {kebab_to_pascal(case)},")
    lines.append("}")
    return "\n".join(lines)


def emit_trait(kind: str, interface: Interface) -> str:
    trait_name = kebab_to_pascal(kind) + kebab_to_pascal(interface.name)
    lines = [
        f"/// Generated {kind.lower()} binding for WIT interface `{interface.name}`.",
        f"pub trait {trait_name} {{",
    ]
    for func in interface.funcs:
        params = ", ".join(
            f"{kebab_to_snake(param.name)}: {rust_param_type(param.ty)}"
            for param in func.params
        )
        signature = (
            f"fn {kebab_to_snake(func.name)}(&self, {params}) -> {rust_type(func.result)};"
            if params
            else f"fn {kebab_to_snake(func.name)}(&self) -> {rust_type(func.result)};"
        )
        lines.append(f"    /// WIT function `{func.name}`.")
        lines.append("    ///")
        lines.append("    /// # Errors")
        lines.append("    ///")
        lines.append(
            "    /// Returns [`PluginError`] when the binding rejects the call."
        )
        lines.append(f"    {signature}")
    lines.append("}")
    return "\n".join(lines)


def emit_source(packages: list[Package]) -> str:
    version = crate_version()
    records: dict[str, list[Field]] = {}
    enums: dict[str, list[str]] = {}
    host_traits: list[Interface] = []
    guest_traits: list[Interface] = []
    worlds: list[World] = []
    host_imports: list[str] = []
    for package in packages:
        for interface in package.interfaces:
            records.update(interface.records)
            enums.update(interface.enums)
            if package.name.startswith("finstack:ai-host"):
                host_traits.append(interface)
            elif interface.funcs:
                guest_traits.append(interface)
        worlds.extend(package.worlds)
        for world in package.worlds:
            host_imports.extend(world.imports)

    toolset_world = next(world for world in worlds if world.name == "toolset-plugin")
    context_world = next(world for world in worlds if world.name == "context-plugin")
    toolset = next(
        interface for interface in guest_traits if interface.name == "toolset"
    )
    context = next(
        interface
        for interface in guest_traits
        if interface.name == "context-provider"
    )
    unique_imports = tuple(dict.fromkeys(host_imports))
    blocks = [
        "// @generated by tools/wit_bindgen/generate.py. Do not hand-edit.",
        "",
        "//! Generated host and guest bindings for the experimental `@0.0.4` WIT packages.",
        "",
        f"/// Workspace crate version that owns these experimental `@0.0.4` bindings (`{version}`).",
        f'pub const CRATE_VERSION: &str = "{version}";',
    ]
    for package in packages:
        const_name = (
            package.name.split(":")[1].split("@")[0].replace("-", "_").upper()
            + "_PACKAGE"
        )
        blocks.append(f"/// Checked-in WIT package `{package.name}`.")
        blocks.append(f'pub const {const_name}: &str = "{package.name}";')
    blocks.extend(
        [
            "/// Worlds published by this experimental plugin alpha.",
            f"pub const PUBLISHED_PACKAGES: &[&str] = &{python_str_array(ALLOWED_PACKAGES)};",
            "/// Host imports granted by experimental plugin worlds. Linking them is not ambient WASI.",
            f"pub const HOST_IMPORTS: &[&str] = &{python_str_array(unique_imports)};",
            "/// Guest exports of `toolset-plugin`.",
            f"pub const TOOLSET_WORLD_EXPORTS: &[&str] = &{python_str_array(tuple(toolset_world.exports))};",
            "/// Functions exported by the `toolset` interface.",
            f"pub const TOOLSET_FUNCS: &[&str] = &{python_str_array(tuple(func.name for func in toolset.funcs))};",
            "/// Guest exports of `context-plugin`.",
            f"pub const CONTEXT_WORLD_EXPORTS: &[&str] = &{python_str_array(tuple(context_world.exports))};",
            "/// Functions exported by the `context-provider` interface.",
            f"pub const CONTEXT_FUNCS: &[&str] = &{python_str_array(tuple(func.name for func in context.funcs))};",
            "/// Tokens that must not appear as nested-agent or lineage operations.",
            f"pub const FORBIDDEN_WORLD_TOKENS: &[&str] = &{python_str_array(FORBIDDEN_WORLD_TOKENS)};",
        ]
    )
    for name, fields in records.items():
        blocks.append(emit_record(name, fields))
    for name, cases in enums.items():
        blocks.append(emit_enum(name, cases))
    for interface in host_traits:
        blocks.append(emit_trait("Host", interface))
    for interface in guest_traits:
        blocks.append(emit_trait("Guest", interface))
    return "\n".join(blocks) + "\n"


def python_str_array(values: tuple[str, ...]) -> str:
    inner = ", ".join(f'"{value}"' for value in values)
    return f"[{inner}]"


def load_packages() -> list[Package]:
    roots = (
        WIT_ROOT / "finstack-ai-types" / "types.wit",
        WIT_ROOT / "finstack-ai-host" / "host.wit",
        WIT_ROOT / "finstack-ai-toolset" / "toolset.wit",
        CONTEXT_WIT,
    )
    for path in roots:
        if not path.is_file():
            raise WitError(f"missing WIT root {path}")
    return [parse_package(path) for path in roots]


def rustfmt(source: str) -> str:
    with tempfile.NamedTemporaryFile(
        "w", encoding="utf-8", suffix=".rs", delete=False
    ) as handle:
        handle.write(source)
        path = Path(handle.name)
    try:
        completed = subprocess.run(
            ["rustfmt", "--edition", "2024", str(path)],
            check=False,
            capture_output=True,
            text=True,
        )
        if completed.returncode != 0:
            raise WitError(completed.stderr or "rustfmt failed")
        return path.read_text(encoding="utf-8")
    finally:
        path.unlink(missing_ok=True)


def write_generated(source: str) -> None:
    GENERATED_RS.parent.mkdir(parents=True, exist_ok=True)
    GENERATED_RS.write_text(source, encoding="utf-8")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate in memory and fail when generated.rs drifts",
    )
    args = parser.parse_args()
    try:
        source = rustfmt(emit_source(load_packages()))
        if args.check:
            current = (
                GENERATED_RS.read_text(encoding="utf-8")
                if GENERATED_RS.exists()
                else ""
            )
            if current != source:
                raise WitError("generated.rs is dirty; run `mise run gen-wit`")
            return 0
        write_generated(source)
        return 0
    except WitError as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
