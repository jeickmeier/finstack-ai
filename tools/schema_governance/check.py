#!/usr/bin/env python3
"""ADR and schema governance enforcement for finstack-ai.

Thin entry point intended for ``mise run schema-governance``. Uses only the
Python standard library. No Cargo xtask.
"""

from __future__ import annotations

import argparse
import json
import os
import re
import subprocess
import sys
import tomllib
from collections.abc import Sequence
from dataclasses import dataclass
from pathlib import Path

TOOL_DIR = Path(__file__).resolve().parent
REPO_ROOT = TOOL_DIR.parents[1]

CHECK_RULES: dict[str, tuple[str, ...]] = {
    "GOV001": ("PR-004-A01", "ENG-COMP"),
    "GOV002": ("PR-004-A01",),
    "GOV003": ("PR-004-A05", "TM-18"),
    "GOV004": ("PR-004-A02", "ENG-COMP"),
    "GOV005": ("PR-004-A04",),
    "GOV006": ("PR-004-A04",),
    "GOV007": ("PR-004-A03", "PLAN-6.1"),
}

REQUIRED_ADR_HEADINGS: tuple[str, ...] = (
    "Status",
    "Date",
    "Accountable role",
    "Decision owners",
    "Context",
    "Decision",
    "Consequences",
    "Rejected alternatives",
    "Compatibility and schema-change classification",
    "Security classification",
    "Affected requirements, design, and delivery",
    "Supersession metadata",
    "Reconsideration conditions",
    "Approval and implementation-evidence links",
)

REQUIRED_CONTRACT_FAMILIES: tuple[str, ...] = (
    "public-rust-api",
    "agent-spec",
    "journal",
    "runtime-events",
    "remote",
    "process",
    "wit",
    "golden-trace",
    "benchmark-report",
)

REQUIRED_PR_IMPACT_HEADINGS: tuple[str, ...] = (
    "API impact",
    "Schema / compatibility impact",
    "Performance / allocation impact",
    "Security / trust-boundary impact",
)

SCHEMA_FILE_RE = re.compile(
    r"^schemas/(?P<family>[a-z0-9-]+)/v(?P<major>\d+)/(?P<kind>[a-z0-9-]+)\.schema\.json$"
)
WIT_SCHEMA_FILE_RE = re.compile(
    r"^plugins/finstack-ai-wit/wit/v(?P<version>\d+\.\d+\.\d+)/.+\.wit$"
)
# Version segment is either a major integer (JSON Schema families) or a
# semver triple (WIT packages under plugins/finstack-ai-wit/wit/vX.Y.Z/).
_VERSION = r"(?P<version>\d+(?:\.\d+\.\d+)?)"
FIXTURE_FILE_RE = re.compile(
    r"^fixtures/compatibility/(?P<family>[a-z0-9-]+)/"
    r"(?:"
    rf"v{_VERSION}/"
    r"(?P<kind>[a-z0-9-]+)/"
    r"(?P<case>valid|invalid|roundtrip)--(?P<slug>[a-z0-9-]+)\.[a-z0-9.]+"
    r"|"
    r"migrations/v(?P<from>\d+(?:\.\d+\.\d+)?)-to-v(?P<to>\d+(?:\.\d+\.\d+)?)--"
    r"(?P<mslug>[a-z0-9-]+)\.(?P<side>before|after)\.json"
    r")$"
)

ADR_FILENAME_RE = re.compile(r"^ADR-(?P<number>\d{3})-(?P<topic>[a-z0-9-]+)\.md$")
ADR_INDEX_ROW_RE = re.compile(
    r"^\|\s*(ADR-(?P<number>\d{3}))\s*\|\s*`(?P<topic>[a-z0-9-]+)`\s*\|"
)

JSON_SCHEMA_2020_12 = "https://json-schema.org/draft/2020-12/schema"


@dataclass(frozen=True, order=True)
class Diagnostic:
    """One governance failure."""

    check_id: str
    subject: str
    message: str
    path: str = ""

    def format(self) -> str:
        location = f" ({self.path})" if self.path else ""
        return f"{self.check_id} {self.subject}{location}: {self.message}"


@dataclass(frozen=True, order=True)
class ContractKey:
    """A governed contract identity used for schema/fixture coupling."""

    family: str
    version: str


def parse_schema_families(path: Path) -> dict[str, dict[str, str]]:
    """Load schema-families.toml into a family-id -> fields mapping."""
    data = tomllib.loads(path.read_text(encoding="utf-8"))
    families = data.get("families", {})
    if not isinstance(families, dict):
        return {}
    result: dict[str, dict[str, str]] = {}
    for family_id, fields in families.items():
        if isinstance(fields, dict):
            result[str(family_id)] = {str(k): str(v) for k, v in fields.items()}
    return result


def _heading_set(text: str) -> set[str]:
    return {line[3:].strip() for line in text.splitlines() if line.startswith("## ")}


def _parse_adr_index(register_text: str) -> dict[int, str]:
    topics: dict[int, str] = {}
    in_index = False
    for line in register_text.splitlines():
        if line.startswith("## Decision and implementation index"):
            in_index = True
            continue
        if in_index and line.startswith("## "):
            break
        match = ADR_INDEX_ROW_RE.match(line)
        if match:
            topics[int(match.group("number"))] = match.group("topic")
    return topics


def _register_links(register_text: str) -> dict[int, str]:
    links: dict[int, str] = {}
    in_links = False
    for line in register_text.splitlines():
        if line.startswith("## Current record and evidence links"):
            in_links = True
            continue
        if in_links and line.startswith("## "):
            break
        match = re.match(
            r"^\|\s*ADR-(?P<number>\d{3})\s*\|\s*"
            r"(?:\[.*?\]\((?P<link>[^)]+)\)|(?P<missing>—[^|]*))\s*\|",
            line,
        )
        if match:
            number = int(match.group("number"))
            link = match.group("link")
            if link:
                links[number] = link
    return links


def check_adr_inventory(repo_root: Path) -> list[Diagnostic]:
    """GOV001: ADR-001..037 files exist and are linked from the register."""
    diagnostics: list[Diagnostic] = []
    register_path = repo_root / "docs/implementation/adr-register.md"
    adrs_dir = repo_root / "docs/implementation/adrs"
    if not register_path.is_file():
        return [
            Diagnostic(
                "GOV001",
                "adr-register",
                "ADR register is missing",
                str(register_path),
            )
        ]
    register_text = register_path.read_text(encoding="utf-8")
    topics = _parse_adr_index(register_text)
    links = _register_links(register_text)
    for number in range(1, 38):
        if number not in topics:
            diagnostics.append(
                Diagnostic(
                    "GOV001",
                    f"ADR-{number:03d}",
                    "missing from ADR register index",
                    str(register_path),
                )
            )
            continue
        topic = topics[number]
        expected = f"ADR-{number:03d}-{topic}.md"
        path = adrs_dir / expected
        if not path.is_file():
            diagnostics.append(
                Diagnostic(
                    "GOV001",
                    f"ADR-{number:03d}",
                    f"standalone record missing: {expected}",
                    str(path),
                )
            )
        link = links.get(number)
        if not link or expected not in link:
            diagnostics.append(
                Diagnostic(
                    "GOV001",
                    f"ADR-{number:03d}",
                    "register current-record link missing or incorrect",
                    str(register_path),
                )
            )
    if adrs_dir.is_dir():
        for path in sorted(adrs_dir.glob("ADR-*.md")):
            match = ADR_FILENAME_RE.match(path.name)
            if not match:
                diagnostics.append(
                    Diagnostic(
                        "GOV001",
                        path.name,
                        "ADR filename does not match ADR-NNN-topic.md",
                        str(path),
                    )
                )
                continue
            number = int(match.group("number"))
            topic = match.group("topic")
            if topics.get(number) != topic:
                diagnostics.append(
                    Diagnostic(
                        "GOV001",
                        path.name,
                        "filename topic does not match ADR register topic key",
                        str(path),
                    )
                )
    return diagnostics


def check_adr_records(repo_root: Path) -> list[Diagnostic]:
    """GOV002/GOV003: required headings and security disposition."""
    diagnostics: list[Diagnostic] = []
    register_path = repo_root / "docs/implementation/adr-register.md"
    adrs_dir = repo_root / "docs/implementation/adrs"
    if not register_path.is_file() or not adrs_dir.is_dir():
        return diagnostics
    topics = _parse_adr_index(register_path.read_text(encoding="utf-8"))
    for number, topic in sorted(topics.items()):
        path = adrs_dir / f"ADR-{number:03d}-{topic}.md"
        if not path.is_file():
            continue
        text = path.read_text(encoding="utf-8")
        headings = _heading_set(text)
        for heading in REQUIRED_ADR_HEADINGS:
            if heading not in headings:
                diagnostics.append(
                    Diagnostic(
                        "GOV002",
                        f"ADR-{number:03d}",
                        f"missing required heading: {heading}",
                        str(path),
                    )
                )
        if number >= 15 and number <= 24:
            if "PRD" not in text and "product-requirements" not in text:
                diagnostics.append(
                    Diagnostic(
                        "GOV002",
                        f"ADR-{number:03d}",
                        "ADR-015–ADR-024 must cross-link PRD §18 delivery detail",
                        str(path),
                    )
                )
        if number >= 29:
            recon = re.search(
                r"## Reconsideration conditions\n+(?P<body>.*?)(?:\n## |\Z)",
                text,
                re.S,
            )
            body = recon.group("body").strip() if recon else ""
            if len(body) < 20:
                diagnostics.append(
                    Diagnostic(
                        "GOV002",
                        f"ADR-{number:03d}",
                        "ADR-029–ADR-037 must record a concrete reconsideration gate",
                        str(path),
                    )
                )
        affected = re.search(
            r"## Affected requirements, design, and delivery\n+(?P<body>.*?)(?:\n## |\Z)",
            text,
            re.S,
        )
        affected_body = affected.group("body") if affected else ""
        if "FR-" not in affected_body and "NFR-" not in affected_body:
            diagnostics.append(
                Diagnostic(
                    "GOV002",
                    f"ADR-{number:03d}",
                    "must cite affected FR-/NFR- requirement families",
                    str(path),
                )
            )
        if "Technical Design" not in affected_body and "TDD" not in affected_body:
            diagnostics.append(
                Diagnostic(
                    "GOV002",
                    f"ADR-{number:03d}",
                    "must cite affected Technical Design sections",
                    str(path),
                )
            )
        security_ok = (
            (
                "SEC-INV-" in text
                or "TM-" in text
                or "No direct security control" in text
            )
            and "Threat Model" in text
            and (
                "Control-change disposition" in text
                or "control-change disposition" in text.lower()
            )
            and "Pending review" not in text
        )
        if not security_ok:
            diagnostics.append(
                Diagnostic(
                    "GOV003",
                    f"ADR-{number:03d}",
                    "security classification must cite SEC-INV/TM or "
                    "'No direct security control', link Threat Model §18, "
                    "and record a control-change disposition",
                    str(path),
                )
            )
    return diagnostics


def check_contract_registry(repo_root: Path) -> list[Diagnostic]:
    """GOV004: every contract family has owner, promise, and fixture root."""
    diagnostics: list[Diagnostic] = []
    registry_path = repo_root / "schemas/schema-families.toml"
    if not registry_path.is_file():
        return [
            Diagnostic(
                "GOV004",
                "schema-families",
                "schemas/schema-families.toml is missing",
                str(registry_path),
            )
        ]
    families = parse_schema_families(registry_path)
    required_fields = (
        "owner",
        "reviewer",
        "source_root",
        "format",
        "stability",
        "compatibility_profile",
        "fixture_root",
        "status",
    )
    for family_id in REQUIRED_CONTRACT_FAMILIES:
        if family_id not in families:
            diagnostics.append(
                Diagnostic(
                    "GOV004",
                    family_id,
                    "missing from schema-families.toml",
                    str(registry_path),
                )
            )
            continue
        fields = families[family_id]
        for field_name in required_fields:
            if not fields.get(field_name):
                diagnostics.append(
                    Diagnostic(
                        "GOV004",
                        family_id,
                        f"missing field: {field_name}",
                        str(registry_path),
                    )
                )
        source_root = fields.get("source_root", "")
        fixture_root = fields.get("fixture_root", "")
        if source_root:
            source_path = repo_root / source_root
            if not source_path.exists():
                diagnostics.append(
                    Diagnostic(
                        "GOV004",
                        family_id,
                        f"source_root does not exist: {source_root}",
                        source_root,
                    )
                )
        if fixture_root:
            fixture_path = repo_root / fixture_root
            if not fixture_path.exists():
                diagnostics.append(
                    Diagnostic(
                        "GOV004",
                        family_id,
                        f"fixture_root does not exist: {fixture_root}",
                        fixture_root,
                    )
                )
    for family_id in sorted(set(families) - set(REQUIRED_CONTRACT_FAMILIES)):
        diagnostics.append(
            Diagnostic(
                "GOV004",
                family_id,
                "unknown contract family not in REQUIRED_CONTRACT_FAMILIES",
                str(registry_path),
            )
        )
    return diagnostics


def _git_name_status(
    repo_root: Path, base_ref: str
) -> list[tuple[str, str, str | None]]:
    """Return (status, path, rename_source) tuples for changes since base_ref."""
    completed = subprocess.run(
        ["git", "diff", "--name-status", "-M", base_ref],
        cwd=repo_root,
        check=False,
        capture_output=True,
        text=True,
    )
    if completed.returncode != 0:
        raise RuntimeError(completed.stderr.strip() or "git diff failed")
    rows: list[tuple[str, str, str | None]] = []
    for line in completed.stdout.splitlines():
        if not line.strip():
            continue
        parts = line.split("\t")
        status = parts[0]
        if status.startswith("R") and len(parts) >= 3:
            rows.append((status, parts[2], parts[1]))
        elif len(parts) >= 2:
            rows.append((status, parts[1], None))
    return rows


def _contract_key_from_schema_path(path: str) -> ContractKey | None:
    match = SCHEMA_FILE_RE.match(path)
    if match:
        return ContractKey(match.group("family"), match.group("major"))
    wit = WIT_SCHEMA_FILE_RE.match(path)
    if wit:
        return ContractKey("wit", wit.group("version"))
    return None


def _contract_keys_from_fixture_path(path: str) -> set[ContractKey]:
    match = FIXTURE_FILE_RE.match(path)
    if not match:
        return set()
    family = match.group("family")
    version = match.groupdict().get("version")
    if version:
        return {ContractKey(family, version)}
    # Migration fixtures cover both from and to versions.
    return {
        ContractKey(family, match.group("from")),
        ContractKey(family, match.group("to")),
    }


def _validate_json_schema_draft(path: Path, rel: str) -> list[Diagnostic]:
    diagnostics: list[Diagnostic] = []
    try:
        data = json.loads(path.read_text(encoding="utf-8"))
    except json.JSONDecodeError as exc:
        return [
            Diagnostic(
                "GOV005",
                rel,
                f"schema JSON is invalid: {exc}",
                rel,
            )
        ]
    if not isinstance(data, dict):
        return [
            Diagnostic(
                "GOV005",
                rel,
                "schema JSON must be an object with $schema",
                rel,
            )
        ]
    schema_uri = data.get("$schema")
    if schema_uri != JSON_SCHEMA_2020_12:
        diagnostics.append(
            Diagnostic(
                "GOV005",
                rel,
                f"$schema must be exactly {JSON_SCHEMA_2020_12}",
                rel,
            )
        )
    return diagnostics


def check_schema_fixture_coupling(
    repo_root: Path,
    base_ref: str | None = None,
) -> list[Diagnostic]:
    """GOV005/GOV006: naming rules and schema/fixture coupling on diffs."""
    diagnostics: list[Diagnostic] = []
    effective_base = (
        base_ref if base_ref is not None else os.environ.get("SCHEMA_GOVERNANCE_BASE")
    )
    registry_path = repo_root / "schemas/schema-families.toml"
    registered_families = (
        set(parse_schema_families(registry_path)) if registry_path.is_file() else set()
    )

    # Static naming checks for currently present schema files.
    schemas_root = repo_root / "schemas"
    if schemas_root.exists():
        for path in sorted(schemas_root.rglob("*.schema.json")):
            rel = path.relative_to(repo_root).as_posix()
            if not SCHEMA_FILE_RE.match(rel):
                diagnostics.append(
                    Diagnostic(
                        "GOV005",
                        rel,
                        "schema path must match "
                        "schemas/<family>/v<major>/<kind>.schema.json",
                        rel,
                    )
                )
            diagnostics.extend(_validate_json_schema_draft(path, rel))

    wit_root = repo_root / "plugins/finstack-ai-wit/wit"
    if wit_root.exists():
        for path in sorted(wit_root.rglob("*.wit")):
            rel = path.relative_to(repo_root).as_posix()
            if not WIT_SCHEMA_FILE_RE.match(rel):
                diagnostics.append(
                    Diagnostic(
                        "GOV005",
                        rel,
                        "WIT path must match "
                        "plugins/finstack-ai-wit/wit/v<x.y.z>/.../*.wit",
                        rel,
                    )
                )

    if not effective_base:
        return diagnostics

    try:
        changes = _git_name_status(repo_root, effective_base)
    except (RuntimeError, FileNotFoundError) as exc:
        diagnostics.append(
            Diagnostic(
                "GOV006",
                "git-diff",
                f"unable to compute schema/fixture diff against {effective_base}: {exc}",
            )
        )
        return diagnostics

    changed_schema_keys: set[ContractKey] = set()
    changed_fixture_keys: set[ContractKey] = set()

    def consider_schema_path(path: str) -> None:
        nonlocal diagnostics
        if path.endswith(".schema.json"):
            key = _contract_key_from_schema_path(path)
            if key is None:
                diagnostics.append(
                    Diagnostic(
                        "GOV005",
                        path,
                        "changed schema path must match "
                        "schemas/<family>/v<major>/<kind>.schema.json",
                        path,
                    )
                )
                return
            if registered_families and key.family not in registered_families:
                diagnostics.append(
                    Diagnostic(
                        "GOV006",
                        key.family,
                        "schema family is not registered in "
                        "schemas/schema-families.toml",
                        path,
                    )
                )
            changed_schema_keys.add(key)
            return
        if path.startswith("plugins/finstack-ai-wit/wit/") and path.endswith(".wit"):
            key = _contract_key_from_schema_path(path)
            if key is None:
                diagnostics.append(
                    Diagnostic(
                        "GOV005",
                        path,
                        "changed WIT path must match "
                        "plugins/finstack-ai-wit/wit/v<x.y.z>/.../*.wit",
                        path,
                    )
                )
                return
            if registered_families and key.family not in registered_families:
                diagnostics.append(
                    Diagnostic(
                        "GOV006",
                        key.family,
                        "schema family is not registered in "
                        "schemas/schema-families.toml",
                        path,
                    )
                )
            changed_schema_keys.add(key)

    def consider_fixture_path(path: str, *, validate_name: bool = True) -> None:
        nonlocal diagnostics
        if not path.startswith("fixtures/compatibility/"):
            return
        # Ignore reserved README scaffolding.
        if path.endswith("/README.md") or path.endswith("README.md"):
            return
        keys = _contract_keys_from_fixture_path(path)
        if not keys:
            if not validate_name:
                return
            diagnostics.append(
                Diagnostic(
                    "GOV005",
                    path,
                    "fixture path must match "
                    "fixtures/compatibility/<family>/v<major>/<kind>/"
                    "<valid|invalid|roundtrip>--<slug>.<ext> or "
                    "fixtures/compatibility/<family>/migrations/"
                    "v<from>-to-v<to>--<slug>.{before,after}.json",
                    path,
                )
            )
            return
        changed_fixture_keys.update(keys)

    for status, path, rename_source in changes:
        consider_schema_path(path)
        consider_fixture_path(path, validate_name=not status.startswith("D"))
        if rename_source:
            consider_schema_path(rename_source)
            # Historical source names can be the defect the change corrects.
            # Keep valid source keys for coupling, but only enforce naming on
            # fixture paths that remain in the candidate tree.
            consider_fixture_path(rename_source, validate_name=False)

    for key in sorted(changed_schema_keys):
        if key not in changed_fixture_keys:
            diagnostics.append(
                Diagnostic(
                    "GOV006",
                    f"{key.family}@v{key.version}",
                    "schema change requires a fixture update for the same "
                    f"family/version under fixtures/compatibility/{key.family}/",
                )
            )
    return diagnostics


def check_pr_template(repo_root: Path) -> list[Diagnostic]:
    """GOV007: PR template requires explicit impact sections."""
    path = repo_root / ".github/PULL_REQUEST_TEMPLATE.md"
    if not path.is_file():
        return [
            Diagnostic(
                "GOV007",
                "pull-request-template",
                "PR template is missing",
                str(path),
            )
        ]
    text = path.read_text(encoding="utf-8")
    diagnostics: list[Diagnostic] = []
    for heading in REQUIRED_PR_IMPACT_HEADINGS:
        if f"## {heading}" not in text:
            diagnostics.append(
                Diagnostic(
                    "GOV007",
                    heading,
                    f"PR template missing required section: ## {heading}",
                    str(path),
                )
            )
    return diagnostics


def run_checks(
    repo_root: Path,
    base_ref: str | None = None,
) -> list[Diagnostic]:
    """Run all governance checks and return diagnostics."""
    diagnostics: list[Diagnostic] = []
    diagnostics.extend(check_adr_inventory(repo_root))
    diagnostics.extend(check_adr_records(repo_root))
    diagnostics.extend(check_contract_registry(repo_root))
    diagnostics.extend(check_schema_fixture_coupling(repo_root, base_ref=base_ref))
    diagnostics.extend(check_pr_template(repo_root))
    return sorted(diagnostics)


def main(argv: Sequence[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--repo-root",
        type=Path,
        default=REPO_ROOT,
        help="Repository root (default: detected from this file)",
    )
    parser.add_argument(
        "--base",
        default=os.environ.get("SCHEMA_GOVERNANCE_BASE"),
        help="Git base revision for schema/fixture coupling (or SCHEMA_GOVERNANCE_BASE)",
    )
    args = parser.parse_args(argv)
    diagnostics = run_checks(args.repo_root.resolve(), base_ref=args.base)
    if not diagnostics:
        print("schema-governance: ok")
        return 0
    for diagnostic in diagnostics:
        print(diagnostic.format(), file=sys.stderr)
    print(f"schema-governance: {len(diagnostics)} finding(s)", file=sys.stderr)
    return 1


if __name__ == "__main__":
    raise SystemExit(main())
