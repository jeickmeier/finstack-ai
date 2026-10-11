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
PYTHON_INIT_FILE = "__init__.py"
PYTHON_INIT = (
    REPO_ROOT
    / "bindings"
    / "finstack-ai-python"
    / "python"
    / "finstack_ai"
    / PYTHON_INIT_FILE
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


def python_subpackage_exports(module: str) -> list[str]:
    return python_exports_at(PYTHON_INIT.parent / module / PYTHON_INIT_FILE)


def python_items() -> list[str]:
    names = python_exports_at(PYTHON_INIT)
    for module in PYTHON_SUBPACKAGES:
        names.extend(f"{module}.{name}" for name in python_subpackage_exports(module))
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
        members[module] = set(python_subpackage_exports(module))
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


def is_known(name: object, known: set[str]) -> bool:
    return isinstance(name, str) and name in known


def portable_function_errors(
    functions: list[object], python_exports: set[str], js_exports: set[str]
) -> list[str]:
    errors: list[str] = []
    for entry in functions:
        if not isinstance(entry, dict):
            errors.append("portable function entry must be an object")
            continue
        python_name = entry.get("python")
        js_name = entry.get("js")
        if not is_known(python_name, python_exports):
            errors.append(f"python portable function missing: {python_name}")
        if not is_known(js_name, js_exports):
            errors.append(f"js portable function missing: {js_name}")
    return errors


def portable_member_errors(
    python_name: str,
    js_name: str,
    expected: object,
    python_known: set[str],
    js_known: set[str],
) -> list[str]:
    if not isinstance(expected, dict):
        return [f"portable member must be an object: {python_name}/{js_name}"]
    errors: list[str] = []
    python_member = expected.get("python")
    js_member = expected.get("js")
    if not is_known(python_member, python_known):
        errors.append(f"python portable member missing: {python_name}.{python_member}")
    if not is_known(js_member, js_known):
        errors.append(f"js portable member missing: {js_name}.{js_member}")
    return errors


def portable_type_errors(
    entry: object,
    python_exports: set[str],
    js_exports: set[str],
    python_members: dict[str, set[str]],
    js_members: dict[str, set[str]],
) -> list[str]:
    if not isinstance(entry, dict):
        return ["portable type entry must be an object"]
    python_name = entry.get("python")
    js_name = entry.get("js")
    if not isinstance(python_name, str) or python_name not in python_exports:
        return [f"python portable type missing: {python_name}"]
    if not isinstance(js_name, str) or js_name not in js_exports:
        return [f"js portable type missing: {js_name}"]
    expected_members = entry.get("members", [])
    if not isinstance(expected_members, list):
        return [f"portable members must be a list: {python_name}/{js_name}"]
    python_known = python_members.get(python_name, set())
    js_known = js_members.get(js_name, set())
    errors: list[str] = []
    for expected in expected_members:
        errors.extend(
            portable_member_errors(
                python_name, js_name, expected, python_known, js_known
            )
        )
    return errors


def parity_errors(inventory: dict[str, object]) -> list[str]:
    python_exports = set(python_items())
    js_exports = set(js_items())
    python_members = python_stub_members()
    js_members = js_class_members()
    functions = inventory.get("functions")
    types = inventory.get("types")
    if not isinstance(functions, list) or not isinstance(types, list):
        return ["portable inventory must contain function and type lists"]
    errors = portable_function_errors(functions, python_exports, js_exports)
    for entry in types:
        errors.extend(
            portable_type_errors(
                entry, python_exports, js_exports, python_members, js_members
            )
        )
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


VALID_BINDING_STATUSES = {
    "direct",
    "composed",
    "facade",
    "host_adapter",
    "unavailable",
}
DEPENDENCY_STATUSES = {"direct", "composed"}
REASON_STATUSES = {"composed", "facade", "host_adapter", "unavailable"}
SURFACE_STATUSES = {"direct", "facade", "host_adapter"}

# (family, direct dependencies, exports, members) for one binding.
BindingSurface = tuple[str, set[str], set[str], dict[str, set[str]]]


def has_rust_baseline(crate: str) -> bool:
    return any(
        path.name == f"{crate}.txt" or path.name.startswith(f"{crate}+")
        for path in RUST_API_BASELINES.glob(f"{crate}*.txt")
    )


def extension_surface_errors(
    crate: str,
    family: str,
    status: str,
    binding: dict[str, object],
    exports: set[str],
    members: dict[str, set[str]],
) -> list[str]:
    surfaces = binding.get("surfaces")
    if (
        not isinstance(surfaces, list)
        or not surfaces
        or not all(isinstance(surface, str) for surface in surfaces)
    ):
        return [f"{crate}: {family} {status} status requires string surfaces"]
    if family == "js" and status == "host_adapter":
        source = binding.get("source")
        if not isinstance(source, str) or not (REPO_ROOT / source).is_file():
            return [f"{crate}: JavaScript host adapter source is missing: {source}"]
        exports = js_declared_items(REPO_ROOT / source)
        members = {}
    return [
        f"{crate}: {family} extension surface missing: {surface}"
        for surface in surfaces
        if not surface_exists(surface, exports, members)
    ]


def extension_binding_errors(
    crate: str, binding: object, surface: BindingSurface
) -> list[str]:
    family, dependencies, exports, members = surface
    if not isinstance(binding, dict):
        return [f"{crate}: missing {family} binding disposition"]
    status = binding.get("status")
    if status not in VALID_BINDING_STATUSES:
        return [f"{crate}: invalid {family} binding status: {status}"]
    errors: list[str] = []
    if status in DEPENDENCY_STATUSES and crate not in dependencies:
        errors.append(
            f"{crate}: {family} {status} extension is not a direct dependency"
        )
    if status == "unavailable" and crate in dependencies:
        errors.append(f"{crate}: {family} dependency cannot be marked unavailable")
    reason = binding.get("reason")
    if status in REASON_STATUSES and (not isinstance(reason, str) or not reason):
        errors.append(f"{crate}: {family} {status} status requires a reason")
    if status in SURFACE_STATUSES:
        errors.extend(
            extension_surface_errors(crate, family, status, binding, exports, members)
        )
    return errors


def extension_entry_errors(
    crate: str, entry: dict[str, object], surfaces: tuple[BindingSurface, ...]
) -> list[str]:
    errors: list[str] = []
    if not has_rust_baseline(crate):
        errors.append(f"Rust public-API baseline missing for extension: {crate}")
    for surface in surfaces:
        errors.extend(extension_binding_errors(crate, entry.get(surface[0]), surface))
    return errors


def extension_catalog_coverage_errors(
    expected_crates: set[str], catalog_crates: set[str]
) -> list[str]:
    missing = sorted(expected_crates - catalog_crates)
    extra = sorted(catalog_crates - expected_crates)
    errors: list[str] = []
    if missing:
        errors.append(f"extension parity catalog missing crates: {', '.join(missing)}")
    if extra:
        errors.append(
            f"extension parity catalog has unknown crates: {', '.join(extra)}"
        )
    return errors


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
    surfaces: tuple[BindingSurface, ...] = (
        ("python", python_dependencies, python_exports, python_members),
        ("js", js_dependencies, js_exports, js_members),
    )
    catalog_crates: set[str] = set()
    errors: list[str] = []

    for entry in entries:
        if not isinstance(entry, dict) or not isinstance(entry.get("crate"), str):
            errors.append("extension parity entry must have a string crate")
            continue
        crate = entry["crate"]
        if crate in catalog_crates:
            errors.append(f"duplicate extension parity entry: {crate}")
        catalog_crates.add(crate)
        errors.extend(extension_entry_errors(crate, entry, surfaces))

    errors.extend(extension_catalog_coverage_errors(expected_crates, catalog_crates))
    return errors


def scenario_entry_errors(scenario_id: str, scenario: dict[str, object]) -> list[str]:
    errors: list[str] = []
    fixture = scenario.get("fixture")
    if isinstance(fixture, str) and not (REPO_ROOT / fixture).is_file():
        errors.append(f"binding parity scenario fixture missing: {fixture}")
    for family in ("rust", "python", "js"):
        surfaces = scenario.get(family)
        if not isinstance(surfaces, list) or not surfaces:
            errors.append(f"{scenario_id}: missing {family} surface")
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
        errors.extend(scenario_entry_errors(scenario_id, scenario))
    return errors


def rust_api_failed(write: bool) -> bool:
    rust = subprocess.run(
        [
            sys.executable,
            str(REPO_ROOT / "scripts" / "compat" / "public_api.py"),
            "--write" if write else "--check",
        ],
        cwd=REPO_ROOT,
        check=False,
    )
    return rust.returncode != 0


def write_parity_mutation(inventory: dict[str, object]) -> None:
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
    if not isinstance(renamed, dict) or not isinstance(renamed.get("python"), str):
        raise SystemExit("binding parity inventory has invalid first member")
    renamed["python"] += "Renamed"
    PARITY_MUTATION.write_text(json.dumps(mutation, indent=2) + "\n", encoding="utf-8")


def rename_direct_surface(binding: object) -> bool:
    if not isinstance(binding, dict) or binding.get("status") != "direct":
        return False
    surfaces = binding.get("surfaces")
    if (
        not isinstance(surfaces, list)
        or not surfaces
        or not isinstance(surfaces[0], str)
    ):
        return False
    surfaces[0] += "Renamed"
    return True


def rename_first_direct_surface(extensions: list[object]) -> bool:
    for extension in extensions:
        if not isinstance(extension, dict):
            continue
        for family in ("python", "js"):
            if rename_direct_surface(extension.get(family)):
                return True
    return False


def write_extension_mutation(extension_inventory: dict[str, object]) -> None:
    extension_mutation = copy.deepcopy(extension_inventory)
    extension_entries = extension_mutation.get("extensions")
    if not isinstance(extension_entries, list):
        raise SystemExit("extension parity inventory has no extensions")
    if not rename_first_direct_surface(extension_entries):
        raise SystemExit("extension parity inventory has no direct surface")
    EXTENSION_MUTATION.write_text(
        json.dumps(extension_mutation, indent=2) + "\n", encoding="utf-8"
    )


def parity_mutations_failed() -> bool:
    if not PARITY_MUTATION.is_file() or not EXTENSION_MUTATION.is_file():
        print("binding parity mutation fixture is missing", file=sys.stderr)
        return True
    failed = False
    mutation = json.loads(PARITY_MUTATION.read_text(encoding="utf-8"))
    if not parity_errors(mutation):
        print(
            "binding parity: renamed-member mutation would silently pass",
            file=sys.stderr,
        )
        failed = True
    extension_mutation = json.loads(EXTENSION_MUTATION.read_text(encoding="utf-8"))
    if not extension_parity_errors(extension_mutation):
        print(
            "binding parity: renamed extension surface mutation would silently pass",
            file=sys.stderr,
        )
        failed = True
    return failed


def binding_parity_failed(write: bool) -> bool:
    if (
        not PARITY_INVENTORY.is_file()
        or not PARITY_SCENARIOS.is_file()
        or not EXTENSION_PARITY.is_file()
    ):
        print("binding parity inventories or scenarios are missing", file=sys.stderr)
        return True
    inventory = json.loads(PARITY_INVENTORY.read_text(encoding="utf-8"))
    extension_inventory = json.loads(EXTENSION_PARITY.read_text(encoding="utf-8"))
    errors = parity_errors(inventory)
    errors.extend(extension_parity_errors(extension_inventory))
    errors.extend(scenario_errors())
    for error in errors:
        print(f"binding parity: {error}", file=sys.stderr)
    if write:
        write_parity_mutation(inventory)
        write_extension_mutation(extension_inventory)
        return bool(errors)
    mutations_failed = parity_mutations_failed()
    return bool(errors) or mutations_failed


def write_name_list(family: str, current: list[str]) -> None:
    write_list(BASELINES[family], current)
    mutated = list(current)
    if not mutated:
        raise SystemExit(f"{family}: empty public list")
    mutated[0] = mutated[0] + "Renamed"
    write_list(MUTATIONS[family], mutated)


def name_list_failed(family: str, current: list[str]) -> bool:
    baseline_path = BASELINES[family]
    mutation_path = MUTATIONS[family]
    if not baseline_path.is_file() or not mutation_path.is_file():
        print(f"{family}: missing baseline or mutation fixture", file=sys.stderr)
        return True
    failed = False
    baseline = read_list(baseline_path)
    missing = compare(current, baseline)
    if missing:
        print(f"{family}: removed public items: {', '.join(missing)}", file=sys.stderr)
        failed = True
    extra = added(current, baseline)
    if extra:
        print(f"{family}: added public items: {', '.join(extra)}", file=sys.stderr)
        failed = True
    mutation = read_list(mutation_path)
    if not compare(current, mutation):
        print(f"{family}: renamed-item mutation would silently pass", file=sys.stderr)
        failed = True
    return failed


def name_lists_failed(write: bool) -> bool:
    failed = False
    for family, extract in EXTRACTORS.items():
        current = extract()
        if write:
            write_name_list(family, current)
        elif name_list_failed(family, current):
            failed = True
    return failed


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.write and not args.check:
        parser.error("pass --write or --check")
    failed = rust_api_failed(args.write)
    if binding_parity_failed(args.write):
        failed = True
    # Tracked apart from `failed` so the remediation note below describes the
    # Python/JS name lists only when one of them actually drifted. public_api.py
    # prints its own note for the Rust baselines.
    names_failed = name_lists_failed(args.write)
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
    return 1 if failed or names_failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
