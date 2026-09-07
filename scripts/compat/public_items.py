#!/usr/bin/env python3
"""Extract and compare frozen public-item lists.

Python and JavaScript keep name-list baselines. Rust `pub use` scrape is
retired; `public_api.py` (cargo-public-api) owns Rust signatures.
"""

from __future__ import annotations

import argparse
import ast
import copy
import json
import re
import subprocess
import tomllib
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
PYTHON_INIT = (
    REPO_ROOT
    / "bindings"
    / "finstack-ai-python"
    / "python"
    / "finstack_ai"
    / "__init__.py"
)
JS_INDEX = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "src" / "index.ts"
PYTHON_STUB = (
    REPO_ROOT
    / "bindings"
    / "finstack-ai-python"
    / "python"
    / "finstack_ai"
    / "_finstack_ai.pyi"
)
JS_AGENT = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "src" / "agent.ts"
PARITY_INVENTORY = (
    REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "binding-parity"
    / "v1"
    / "portable-api.json"
)
PARITY_SCENARIOS = PARITY_INVENTORY.with_name("scenarios.json")
PARITY_MUTATION = PARITY_INVENTORY.with_name("invalid--renamed-member.json")
EXTENSION_PARITY = PARITY_INVENTORY.with_name("extensions.json")
EXTENSION_MUTATION = PARITY_INVENTORY.with_name("invalid--extension-surface.json")
PYTHON_CARGO = REPO_ROOT / "bindings" / "finstack-ai-python" / "Cargo.toml"
WASM_CARGO = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "Cargo.toml"
RUST_API_BASELINES = (
    REPO_ROOT / "fixtures" / "compatibility" / "public-rust-api" / "cargo-public-api"
)
BASELINES = {
    "python": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "python"
    / "valid--v0.1.0-exports.txt",
    "js": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "js"
    / "valid--v0.1.0-exports.txt",
}
MUTATIONS = {
    "python": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "python"
    / "invalid--renamed-export.txt",
    "js": REPO_ROOT
    / "fixtures"
    / "compatibility"
    / "breaking"
    / "js"
    / "invalid--renamed-export.txt",
}

JS_EXPORT_RE = re.compile(r"export (?:type )?\{([^}]+)\}", re.S)
JS_FN_RE = re.compile(r"^export (?:async )?function ([A-Za-z_][A-Za-z0-9_]*)", re.M)
JS_CONST_RE = re.compile(r"^export const ([A-Za-z_][A-Za-z0-9_]*)", re.M)
JS_DECL_RE = re.compile(
    r"^export (?:declare )?(?:interface|class|type|enum) "
    r"([A-Za-z_][A-Za-z0-9_]*)",
    re.M,
)


def python_exports_at(path: Path) -> list[str]:
    tree = ast.parse(path.read_text(encoding="utf-8"), filename=str(path))
    for node in tree.body:
        if not isinstance(node, ast.Assign):
            continue
        if not any(
            isinstance(target, ast.Name) and target.id == "__all__"
            for target in node.targets
        ):
            continue
        value = ast.literal_eval(node.value)
        if not isinstance(value, list) or not all(
            isinstance(item, str) for item in value
        ):
            raise ValueError("Python __all__ must be a literal list of strings")
        return sorted(value)
    raise ValueError("Python package does not define __all__")


# Native subpackages are published entrypoints with their own typed exports.
PYTHON_SUBPACKAGES = ("eval", "search")


def python_items() -> list[str]:
    names = python_exports_at(PYTHON_INIT)
    for module in PYTHON_SUBPACKAGES:
        names.extend(
            f"{module}.{name}"
            for name in python_exports_at(PYTHON_INIT.parent / module / "__init__.py")
        )
    return sorted(names)


def js_declared_items(path: Path) -> set[str]:
    text = path.read_text(encoding="utf-8")
    names: set[str] = set()
    for block in JS_EXPORT_RE.findall(text):
        for part in block.split(","):
            ident = part.strip().split(" as ")[-1].strip()
            if ident:
                names.add(ident)
    names.update(JS_FN_RE.findall(text))
    names.update(JS_CONST_RE.findall(text))
    names.update(JS_DECL_RE.findall(text))
    return names


def js_items() -> list[str]:
    return sorted(js_declared_items(JS_INDEX))


EXTRACTORS = {"python": python_items, "js": js_items}


def write_list(path: Path, items: list[str]) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text("\n".join(items) + "\n", encoding="utf-8")


def read_list(path: Path) -> list[str]:
    return [line for line in path.read_text(encoding="utf-8").splitlines() if line]


def compare(current: list[str], baseline: list[str]) -> list[str]:
    missing = [item for item in baseline if item not in set(current)]
    return missing


def added(current: list[str], baseline: list[str]) -> list[str]:
    known = set(baseline)
    return [item for item in current if item not in known]


def python_stub_members() -> dict[str, set[str]]:
    tree = ast.parse(PYTHON_STUB.read_text(encoding="utf-8"), filename=str(PYTHON_STUB))
    members: dict[str, set[str]] = {}
    for node in tree.body:
        if isinstance(node, ast.ClassDef):
            members[node.name] = {
                item.name
                for item in node.body
                if isinstance(item, (ast.FunctionDef, ast.AsyncFunctionDef))
            }
    for module in PYTHON_SUBPACKAGES:
        members[module] = set(
            python_exports_at(PYTHON_INIT.parent / module / "__init__.py")
        )
    return members


def js_class_members() -> dict[str, set[str]]:
    text = JS_AGENT.read_text(encoding="utf-8")
    classes: dict[str, set[str]] = {}
    declaration = re.compile(r"^export class ([A-Za-z_][A-Za-z0-9_]*) \{", re.M)
    member = re.compile(
        r"^  (?:(?:static|async|get) )*([A-Za-z_][A-Za-z0-9_]*)\s*(?:\(|:)",
        re.M,
    )
    for match in declaration.finditer(text):
        start = match.end()
        depth = 1
        index = start
        while index < len(text) and depth:
            character = text[index]
            if character == "{":
                depth += 1
            elif character == "}":
                depth -= 1
            index += 1
        if depth:
            raise ValueError(f"unterminated JavaScript class {match.group(1)}")
        body = text[start : index - 1]
        classes[match.group(1)] = set(member.findall(body))
    return classes


def parity_errors(inventory: dict[str, object]) -> list[str]:
    python_exports = set(python_items())
    js_exports = set(js_items())
    python_members = python_stub_members()
    js_members = js_class_members()
    errors: list[str] = []
    functions = inventory.get("functions")
    types = inventory.get("types")
    if not isinstance(functions, list) or not isinstance(types, list):
        return ["portable inventory must contain function and type lists"]
    for entry in functions:
        if not isinstance(entry, dict):
            errors.append("portable function entry must be an object")
            continue
        python_name = entry.get("python")
        js_name = entry.get("js")
        if not isinstance(python_name, str) or python_name not in python_exports:
            errors.append(f"python portable function missing: {python_name}")
        if not isinstance(js_name, str) or js_name not in js_exports:
            errors.append(f"js portable function missing: {js_name}")
    for entry in types:
        if not isinstance(entry, dict):
            errors.append("portable type entry must be an object")
            continue
        python_name = entry.get("python")
        js_name = entry.get("js")
        if not isinstance(python_name, str) or python_name not in python_exports:
            errors.append(f"python portable type missing: {python_name}")
            continue
        if not isinstance(js_name, str) or js_name not in js_exports:
            errors.append(f"js portable type missing: {js_name}")
            continue
        expected_members = entry.get("members", [])
        if not isinstance(expected_members, list):
            errors.append(f"portable members must be a list: {python_name}/{js_name}")
            continue
        for expected in expected_members:
            if not isinstance(expected, dict):
                errors.append(
                    f"portable member must be an object: {python_name}/{js_name}"
                )
                continue
            python_member = expected.get("python")
            js_member = expected.get("js")
            if not isinstance(
                python_member, str
            ) or python_member not in python_members.get(python_name, set()):
                errors.append(
                    f"python portable member missing: {python_name}.{python_member}"
                )
            if not isinstance(js_member, str) or js_member not in js_members.get(
                js_name, set()
            ):
                errors.append(f"js portable member missing: {js_name}.{js_member}")
    return errors


def extension_crates() -> set[str]:
    crates: set[str] = set()
    for manifest in (REPO_ROOT / "extensions").glob("**/Cargo.toml"):
        document = tomllib.loads(manifest.read_text(encoding="utf-8"))
        package = document.get("package")
        if not isinstance(package, dict) or not isinstance(package.get("name"), str):
            raise ValueError(f"extension manifest has no package name: {manifest}")
        crates.add(package["name"])
    return crates


def cargo_dependencies(path: Path) -> set[str]:
    document = tomllib.loads(path.read_text(encoding="utf-8"))
    dependencies = document.get("dependencies")
    if not isinstance(dependencies, dict):
        raise ValueError(f"binding manifest has no dependencies: {path}")
    return set(dependencies)


def surface_exists(
    surface: str, exports: set[str], members: dict[str, set[str]]
) -> bool:
    owner, separator, member = surface.partition(".")
    if not separator:
        return surface in exports
    return owner in exports and member in members.get(owner, set())


def extension_parity_errors(document: dict[str, object]) -> list[str]:
    entries = document.get("extensions")
    if not isinstance(entries, list):
        return ["extension parity catalog must contain an extension list"]

    expected_crates = extension_crates()
    python_dependencies = cargo_dependencies(PYTHON_CARGO)
    js_dependencies = cargo_dependencies(WASM_CARGO)
    python_exports = set(python_items())
    python_members = python_stub_members()
    js_exports = set(js_items())
    js_members = js_class_members()
    catalog_crates: set[str] = set()
    errors: list[str] = []
    valid_statuses = {"direct", "composed", "facade", "host_adapter", "unavailable"}

    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("crate"), str):
            errors.append("extension parity entry must have a string crate")
            continue
        crate = entry["crate"]
        if crate in catalog_crates:
            errors.append(f"duplicate extension parity entry: {crate}")
        catalog_crates.add(crate)
        if not any(
            path.name == f"{crate}.txt" or path.name.startswith(f"{crate}+")
            for path in RUST_API_BASELINES.glob(f"{crate}*.txt")
        ):
            errors.append(f"Rust public-API baseline missing for extension: {crate}")

        for family, dependencies, exports, members in (
            ("python", python_dependencies, python_exports, python_members),
            ("js", js_dependencies, js_exports, js_members),
        ):
            binding = entry.get(family)
            if not isinstance(binding, dict):
                errors.append(f"{crate}: missing {family} binding disposition")
                continue
            status = binding.get("status")
            if status not in valid_statuses:
                errors.append(f"{crate}: invalid {family} binding status: {status}")
                continue
            if status in {"direct", "composed"} and crate not in dependencies:
                errors.append(
                    f"{crate}: {family} {status} extension is not a direct dependency"
                )
            if status == "unavailable" and crate in dependencies:
                errors.append(
                    f"{crate}: {family} dependency cannot be marked unavailable"
                )
            reason = binding.get("reason")
            if status in {"composed", "facade", "host_adapter", "unavailable"} and (
                not isinstance(reason, str) or not reason
            ):
                errors.append(f"{crate}: {family} {status} status requires a reason")
            surfaces = binding.get("surfaces")
            if status in {"direct", "facade", "host_adapter"}:
                if (
                    not isinstance(surfaces, list)
                    or not surfaces
                    or not all(isinstance(surface, str) for surface in surfaces)
                ):
                    errors.append(
                        f"{crate}: {family} {status} status requires string surfaces"
                    )
                    continue
                source_exports = exports
                source_members = members
                if family == "js" and status == "host_adapter":
                    source = binding.get("source")
                    if (
                        not isinstance(source, str)
                        or not (REPO_ROOT / source).is_file()
                    ):
                        errors.append(
                            f"{crate}: JavaScript host adapter source is missing: {source}"
                        )
                        continue
                    source_exports = js_declared_items(REPO_ROOT / source)
                    source_members = {}
                for surface in surfaces:
                    if not surface_exists(surface, source_exports, source_members):
                        errors.append(
                            f"{crate}: {family} extension surface missing: {surface}"
                        )

    missing = sorted(expected_crates - catalog_crates)
    extra = sorted(catalog_crates - expected_crates)
    if missing:
        errors.append(f"extension parity catalog missing crates: {', '.join(missing)}")
    if extra:
        errors.append(
            f"extension parity catalog has unknown crates: {', '.join(extra)}"
        )
    return errors


def scenario_errors() -> list[str]:
    document = json.loads(PARITY_SCENARIOS.read_text(encoding="utf-8"))
    scenarios = document.get("scenarios")
    if not isinstance(scenarios, list) or not scenarios:
        return ["binding parity scenarios must be a non-empty list"]
    errors: list[str] = []
    seen: set[str] = set()
    for scenario in scenarios:
        if not isinstance(scenario, dict) or not isinstance(scenario.get("id"), str):
            errors.append("binding parity scenario must have a string id")
            continue
        scenario_id = scenario["id"]
        if scenario_id in seen:
            errors.append(f"duplicate binding parity scenario: {scenario_id}")
        seen.add(scenario_id)
        fixture = scenario.get("fixture")
        if isinstance(fixture, str) and not (REPO_ROOT / fixture).is_file():
            errors.append(f"binding parity scenario fixture missing: {fixture}")
        for family in ("rust", "python", "js"):
            surfaces = scenario.get(family)
            if not isinstance(surfaces, list) or not surfaces:
                errors.append(f"{scenario_id}: missing {family} surface")
    return errors


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.write and not args.check:
        parser.error("pass --write or --check")
    rust = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts" / "compat" / "public_api.py"),
            "--write" if args.write else "--check",
        ],
        cwd=REPO_ROOT,
        check=False,
    )
    failed = rust.returncode != 0
    if (
        not PARITY_INVENTORY.is_file()
        or not PARITY_SCENARIOS.is_file()
        or not EXTENSION_PARITY.is_file()
    ):
        print("binding parity inventories or scenarios are missing", file=sys.stderr)
        failed = True
    else:
        inventory = json.loads(PARITY_INVENTORY.read_text(encoding="utf-8"))
        extension_inventory = json.loads(EXTENSION_PARITY.read_text(encoding="utf-8"))
        errors = parity_errors(inventory)
        errors.extend(extension_parity_errors(extension_inventory))
        errors.extend(scenario_errors())
        if errors:
            for error in errors:
                print(f"binding parity: {error}", file=sys.stderr)
            failed = True
        if args.write:
            mutation = copy.deepcopy(inventory)
            types = mutation.get("types")
            if not isinstance(types, list) or not types:
                raise SystemExit("binding parity inventory has no types")
            first = types[0]
            if not isinstance(first, dict):
                raise SystemExit("binding parity inventory has invalid first type")
            members = first.get("members")
            if not isinstance(members, list) or not members:
                raise SystemExit("binding parity inventory has no members")
            renamed = members[0]
            if not isinstance(renamed, dict) or not isinstance(
                renamed.get("python"), str
            ):
                raise SystemExit("binding parity inventory has invalid first member")
            renamed["python"] += "Renamed"
            PARITY_MUTATION.write_text(
                json.dumps(mutation, indent=2) + "\n", encoding="utf-8"
            )
            extension_mutation = copy.deepcopy(extension_inventory)
            extension_entries = extension_mutation.get("extensions")
            if not isinstance(extension_entries, list):
                raise SystemExit("extension parity inventory has no extensions")
            extension_surface_renamed = False
            for extension in extension_entries:
                if not isinstance(extension, dict):
                    continue
                for family in ("python", "js"):
                    binding = extension.get(family)
                    if (
                        not isinstance(binding, dict)
                        or binding.get("status") != "direct"
                    ):
                        continue
                    surfaces = binding.get("surfaces")
                    if (
                        isinstance(surfaces, list)
                        and surfaces
                        and isinstance(surfaces[0], str)
                    ):
                        surfaces[0] += "Renamed"
                        extension_surface_renamed = True
                        break
                if extension_surface_renamed:
                    break
            if not extension_surface_renamed:
                raise SystemExit("extension parity inventory has no direct surface")
            EXTENSION_MUTATION.write_text(
                json.dumps(extension_mutation, indent=2) + "\n", encoding="utf-8"
            )
        elif not PARITY_MUTATION.is_file() or not EXTENSION_MUTATION.is_file():
            print("binding parity mutation fixture is missing", file=sys.stderr)
            failed = True
        else:
            mutation = json.loads(PARITY_MUTATION.read_text(encoding="utf-8"))
            if not parity_errors(mutation):
                print(
                    "binding parity: renamed-member mutation would silently pass",
                    file=sys.stderr,
                )
                failed = True
            extension_mutation = json.loads(
                EXTENSION_MUTATION.read_text(encoding="utf-8")
            )
            if not extension_parity_errors(extension_mutation):
                print(
                    "binding parity: renamed extension surface mutation would silently pass",
                    file=sys.stderr,
                )
                failed = True
    # Tracked apart from `failed` so the remediation note below describes the
    # Python/JS name lists only when one of them actually drifted. public_api.py
    # prints its own note for the Rust baselines.
    names_failed = False
    for family, extract in EXTRACTORS.items():
        current = extract()
        baseline_path = BASELINES[family]
        mutation_path = MUTATIONS[family]
        if args.write:
            write_list(baseline_path, current)
            mutated = list(current)
            if not mutated:
                raise SystemExit(f"{family}: empty public list")
            mutated[0] = mutated[0] + "Renamed"
            write_list(mutation_path, mutated)
            continue
        if not baseline_path.is_file() or not mutation_path.is_file():
            print(f"{family}: missing baseline or mutation fixture", file=sys.stderr)
            failed = True
            names_failed = True
            continue
        baseline = read_list(baseline_path)
        missing = compare(current, baseline)
        if missing:
            print(
                f"{family}: removed public items: {', '.join(missing)}", file=sys.stderr
            )
            failed = True
            names_failed = True
        extra = added(current, baseline)
        if extra:
            print(f"{family}: added public items: {', '.join(extra)}", file=sys.stderr)
            failed = True
            names_failed = True
        mutation = read_list(mutation_path)
        if not compare(current, mutation):
            print(
                f"{family}: renamed-item mutation would silently pass",
                file=sys.stderr,
            )
            failed = True
            names_failed = True
    if names_failed:
        print(
            "\nFrozen public-item lists differ. If the change is intended,"
            " regenerate and commit them:"
            "\n    mise run write-public-api"
            "\nThat rewrites the Python and JavaScript export lists under"
            " fixtures/compatibility/breaking/, and the cargo-public-api"
            " baselines.",
            file=sys.stderr,
        )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
