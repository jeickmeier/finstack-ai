"""Unit tests for ADR and schema governance enforcement."""

from __future__ import annotations

import os
import shutil
import subprocess
import tempfile
from pathlib import Path

from schema_governance.check import (
    CHECK_RULES,
    REQUIRED_ADR_HEADINGS,
    REQUIRED_CONTRACT_FAMILIES,
    REQUIRED_PR_IMPACT_HEADINGS,
    Diagnostic,
    check_adr_inventory,
    check_adr_records,
    check_contract_registry,
    check_pr_template,
    check_schema_fixture_coupling,
    parse_schema_families,
    run_checks,
)

REPO_ROOT = Path(__file__).resolve().parents[3]
TOOL_DIR = REPO_ROOT / "tools" / "schema_governance"

VALID_SCHEMA = '{\n  "$schema": "https://json-schema.org/draft/2020-12/schema",\n  "type": "object"\n}\n'


def _write(path: Path, text: str) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(text, encoding="utf-8")


def _minimal_adr(
    number: int,
    topic: str,
    *,
    security: str = "SEC-INV-001; TM-01",
    disposition: str = "No control change: standalone recording of the accepted planning baseline.",
    reconsideration: str = "Superseding ADR plus primary-document reconciliation.",
    extra: str = "",
    include_traceability: bool = True,
) -> str:
    affected = "## Affected requirements, design, and delivery\n\n- Affected requirements: FR-KRN\n- Affected Technical Design: Technical Design §2\n- Architecture Specification §25\n- Implementation Plan PR-004\n- Planned delivery: PR-004\n"
    if not include_traceability:
        affected = "## Affected requirements, design, and delivery\n\n- Architecture Specification §25\n- Implementation Plan PR-004\n- Planned delivery: PR-004\n"
    return f"# ADR-{number:03d}: {topic}\n\n## Status\n\nAccepted\n\n## Date\n\n2026-08-08\n\n## Accountable role\n\nCore/runtime lead\n\n## Decision owners\n\nme@jeickmeier.com\n\n## Context\n\nBaseline decision captured for governance.\n\n## Decision\n\n{topic} is the accepted direction.\n\n## Consequences\n\nLater PRs must not silently reinterpret this decision.\n\n## Rejected alternatives\n\nCompeting designs were rejected in the planning baseline.\n\n## Compatibility and schema-change classification\n\nPolicy / non-schema decision unless noted.\n\n## Security classification\n\n- References: {security}\n- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)\n- Control-change disposition: {disposition}\n\n{affected}\n## Supersession metadata\n\nNone. This ADR supersedes no prior ADR and is not superseded.\n\n## Reconsideration conditions\n\n{reconsideration}\n\n## Approval and implementation-evidence links\n\n- Approval: accepted in the planning baseline (Architecture Specification §25)\n- Implementation evidence: Missing until mapped delivery work completes\n{extra}\n"


def _register_markdown(topics: dict[int, str]) -> str:
    rows = []
    for number in range(1, 38):
        topic = topics[number]
        rows.append(
            f"| ADR-{number:03d} | `{topic}` | Core/runtime lead | PR-004 | Accepted | Standalone | Not started | Missing |"
        )
    body = "\n".join(rows)
    links = "\n".join(
        (
            f"| ADR-{n:03d} | [ADR-{n:03d}-{topics[n]}.md](adrs/ADR-{n:03d}-{topics[n]}.md) | me@jeickmeier.com | — | PR-004 | 2026-08-08 |"
            for n in range(1, 38)
        )
    )
    return f"# ADR database\n\n## Decision and implementation index\n\n| ADR | Topic key | Accountable role | Planned delivery | Decision | Record | Implementation | Evidence |\n| --- | --- | --- | --- | --- | --- | --- | --- |\n{body}\n\n## Current record and evidence links\n\n| ADR | Standalone record | Assigned to | Current evidence | Change reference | Updated |\n| --- | --- | --- | --- | --- | --- | --- |\n{links}\n"


def _schema_families_toml() -> str:
    lines = ["[families]"]
    for family_id in REQUIRED_CONTRACT_FAMILIES:
        if family_id == "wit":
            source_root = "plugins/finstack-ai-wit/wit/"
            fmt = "wit"
        elif family_id == "public-rust-api":
            source_root = "crates/"
            fmt = "rust-public-api"
        else:
            source_root = f"schemas/{family_id}/"
            fmt = "json-schema-2020-12"
        lines.extend(
            [
                f'[families."{family_id}"]',
                'owner = "me@jeickmeier.com"',
                'reviewer = "me@jeickmeier.com"',
                f'source_root = "{source_root}"',
                f'format = "{fmt}"',
                'stability = "candidate-v1"',
                'compatibility_profile = "strict-reject-unknown"',
                f'fixture_root = "fixtures/compatibility/{family_id}/"',
                'status = "reserved"',
                "",
            ]
        )
    return "\n".join(lines)


def _pr_template() -> str:
    sections = "\n\n".join(
        (f"## {heading}\n\n-" for heading in REQUIRED_PR_IMPACT_HEADINGS)
    )
    return f"## Summary\n\n-\n\n{sections}\n"


def _seed_registry_paths(root: Path) -> None:
    _write(root / "schemas/schema-families.toml", _schema_families_toml())
    _write(root / "crates/README.md", "# crates\n")
    for family_id in REQUIRED_CONTRACT_FAMILIES:
        if family_id == "wit":
            _write(root / "plugins/finstack-ai-wit/wit/README.md", "# WIT\n")
        elif family_id == "public-rust-api":
            _write(root / "schemas/public-rust-api/README.md", "# notes\n")
        else:
            _write(root / "schemas" / family_id / "README.md", f"# {family_id}\n")
        _write(
            root / "fixtures/compatibility" / family_id / "README.md",
            f"# {family_id}\n",
        )


class CheckIdTests:
    def test_stable_check_ids_exist(self) -> None:
        for check_id in (
            "GOV001",
            "GOV002",
            "GOV003",
            "GOV004",
            "GOV005",
            "GOV006",
            "GOV007",
        ):
            assert check_id in CHECK_RULES

    def test_process_family_is_required(self) -> None:
        assert "process" in REQUIRED_CONTRACT_FAMILIES

    def test_pr005_harness_families_are_required(self) -> None:
        assert "golden-trace" in REQUIRED_CONTRACT_FAMILIES
        assert "benchmark-report" in REQUIRED_CONTRACT_FAMILIES


class AdrInventoryTests:
    def test_missing_adr_files_fail_gov001(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            topics = {n: f"topic-{n:03d}" for n in range(1, 38)}
            _write(
                root / "docs/implementation/adr-register.md", _register_markdown(topics)
            )
            _write(root / "docs/implementation/adrs/README.md", "# ADR records\n")
            diagnostics = check_adr_inventory(root)
            assert any((d.check_id == "GOV001" for d in diagnostics))

    def test_complete_inventory_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            topics = {n: f"topic-{n:03d}" for n in range(1, 38)}
            _write(
                root / "docs/implementation/adr-register.md", _register_markdown(topics)
            )
            for number, topic in topics.items():
                path = (
                    root / "docs/implementation/adrs" / f"ADR-{number:03d}-{topic}.md"
                )
                _write(path, _minimal_adr(number, topic))
            diagnostics = check_adr_inventory(root)
            assert not any((d.check_id == "GOV001" for d in diagnostics))


class AdrRecordTests:
    def test_missing_heading_fails_gov002(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            topics = {n: f"topic-{n:03d}" for n in range(1, 38)}
            _write(
                root / "docs/implementation/adr-register.md", _register_markdown(topics)
            )
            for number, topic in topics.items():
                text = _minimal_adr(number, topic).replace("## Consequences\n", "")
                _write(
                    root / "docs/implementation/adrs" / f"ADR-{number:03d}-{topic}.md",
                    text,
                )
            diagnostics = check_adr_records(root)
            assert any((d.check_id == "GOV002" for d in diagnostics))

    def test_missing_traceability_fails_gov002(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            topics = {n: f"topic-{n:03d}" for n in range(1, 38)}
            _write(
                root / "docs/implementation/adr-register.md", _register_markdown(topics)
            )
            for number, topic in topics.items():
                _write(
                    root / "docs/implementation/adrs" / f"ADR-{number:03d}-{topic}.md",
                    _minimal_adr(number, topic, include_traceability=False),
                )
            diagnostics = check_adr_records(root)
            assert any(
                (d.check_id == "GOV002" and "FR-" in d.message for d in diagnostics)
            )
            assert any(
                (
                    d.check_id == "GOV002" and "Technical Design" in d.message
                    for d in diagnostics
                )
            )

    def test_missing_security_fails_gov003(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            topics = {n: f"topic-{n:03d}" for n in range(1, 38)}
            _write(
                root / "docs/implementation/adr-register.md", _register_markdown(topics)
            )
            for number, topic in topics.items():
                text = _minimal_adr(number, topic, security="Pending review")
                _write(
                    root / "docs/implementation/adrs" / f"ADR-{number:03d}-{topic}.md",
                    text,
                )
            diagnostics = check_adr_records(root)
            assert any((d.check_id == "GOV003" for d in diagnostics))

    def test_required_headings_are_documented(self) -> None:
        assert "Decision" in REQUIRED_ADR_HEADINGS
        assert "Security classification" in REQUIRED_ADR_HEADINGS


class ContractRegistryTests:
    def test_missing_family_fails_gov004(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write(
                root / "schemas/schema-families.toml",
                '[families."agent-spec"]\nowner = "me@jeickmeier.com"\nreviewer = "me@jeickmeier.com"\nsource_root = "schemas/agent-spec/"\nformat = "json-schema-2020-12"\nstability = "candidate-v1"\ncompatibility_profile = "strict-reject-unknown"\nfixture_root = "fixtures/compatibility/agent-spec/"\nstatus = "reserved"\n',
            )
            diagnostics = check_contract_registry(root)
            assert any((d.check_id == "GOV004" for d in diagnostics))

    def test_complete_registry_passes(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _seed_registry_paths(root)
            diagnostics = check_contract_registry(root)
            assert not any((d.check_id == "GOV004" for d in diagnostics))
            families = parse_schema_families(root / "schemas/schema-families.toml")
            assert set(families) == set(REQUIRED_CONTRACT_FAMILIES)
            assert families["public-rust-api"]["source_root"] == "crates/"


class SchemaFixtureCouplingTests:
    def _git_repo(self) -> Path:
        root = Path(tempfile.mkdtemp())
        subprocess.run(["git", "init"], cwd=root, check=True, capture_output=True)
        subprocess.run(
            ["git", "config", "user.email", "test@example.com"],
            cwd=root,
            check=True,
            capture_output=True,
        )
        subprocess.run(
            ["git", "config", "user.name", "Test"],
            cwd=root,
            check=True,
            capture_output=True,
        )
        _seed_registry_paths(root)
        subprocess.run(["git", "add", "."], cwd=root, check=True, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", "base"], cwd=root, check=True, capture_output=True
        )
        return root

    def teardown_method(self) -> None:
        root = getattr(self, "_tmp_root", None)
        if root is not None and root.exists():
            shutil.rmtree(root)

    def _head(self, root: Path) -> str:
        return subprocess.check_output(
            ["git", "rev-parse", "HEAD"], cwd=root, text=True
        ).strip()

    def _commit(self, root: Path, message: str) -> None:
        subprocess.run(["git", "add", "-A"], cwd=root, check=True, capture_output=True)
        subprocess.run(
            ["git", "commit", "-m", message], cwd=root, check=True, capture_output=True
        )

    def test_no_schema_files_passes_without_base(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _seed_registry_paths(root)
            diagnostics = check_schema_fixture_coupling(root, base_ref=None)
            assert not any((d.check_id in {"GOV005", "GOV006"} for d in diagnostics))

    def test_schema_change_without_fixture_fails_gov006(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/agent-spec/v1/agent-spec.schema.json", VALID_SCHEMA)
        self._commit(root, "schema only")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (
                d.check_id == "GOV006" and "agent-spec@v1" in d.subject
                for d in diagnostics
            )
        )

    def test_schema_change_with_same_version_fixture_passes(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/agent-spec/v1/agent-spec.schema.json", VALID_SCHEMA)
        _write(
            root
            / "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "{}\n",
        )
        self._commit(root, "schema and fixture")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert not any((d.check_id == "GOV006" for d in diagnostics))

    def test_v2_schema_not_covered_by_v1_fixture(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/agent-spec/v2/agent-spec.schema.json", VALID_SCHEMA)
        _write(
            root
            / "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "{}\n",
        )
        self._commit(root, "cross version mismatch")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (
                d.check_id == "GOV006" and "agent-spec@v2" in d.subject
                for d in diagnostics
            )
        )

    def test_readme_fixture_does_not_satisfy_coupling(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/agent-spec/v1/agent-spec.schema.json", VALID_SCHEMA)
        _write(
            root / "fixtures/compatibility/agent-spec/v1/README.md", "# not a fixture\n"
        )
        self._commit(root, "readme only")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (
                d.check_id == "GOV006" and "agent-spec@v1" in d.subject
                for d in diagnostics
            )
        )

    def test_invalid_schema_name_fails_gov005(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/agent-spec/bad-name.schema.json", VALID_SCHEMA)
        _write(
            root
            / "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "{}\n",
        )
        self._commit(root, "bad schema name")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any((d.check_id == "GOV005" for d in diagnostics))

    def test_wrong_draft_uri_fails_gov005(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        _write(
            root / "schemas/agent-spec/v1/agent-spec.schema.json",
            '{\n  "$schema": "https://json-schema.org/draft-07/schema",\n  "type": "object"\n}\n',
        )
        diagnostics = check_schema_fixture_coupling(root, base_ref=None)
        assert any(
            (d.check_id == "GOV005" and "$schema" in d.message for d in diagnostics)
        )

    def test_comment_containing_draft_uri_without_schema_field_fails(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        _write(
            root / "schemas/agent-spec/v1/agent-spec.schema.json",
            '{\n  "description": "mentions https://json-schema.org/draft/2020-12/schema",\n  "type": "object"\n}\n',
        )
        diagnostics = check_schema_fixture_coupling(root, base_ref=None)
        assert any(
            (d.check_id == "GOV005" and "$schema" in d.message for d in diagnostics)
        )

    def test_unregistered_family_fails_gov006(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "schemas/mystery/v1/thing.schema.json", VALID_SCHEMA)
        _write(
            root / "fixtures/compatibility/mystery/v1/thing/valid--minimal.json", "{}\n"
        )
        self._commit(root, "unregistered family")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (
                d.check_id == "GOV006"
                and "not registered" in d.message
                and (d.subject == "mystery")
                for d in diagnostics
            )
        )

    def test_schema_delete_requires_fixture_update(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        _write(root / "schemas/agent-spec/v1/agent-spec.schema.json", VALID_SCHEMA)
        _write(
            root
            / "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "{}\n",
        )
        self._commit(root, "add schema")
        base = self._head(root)
        (root / "schemas/agent-spec/v1/agent-spec.schema.json").unlink()
        self._commit(root, "delete schema only")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (
                d.check_id == "GOV006" and "agent-spec@v1" in d.subject
                for d in diagnostics
            )
        )

    def test_schema_rename_source_requires_fixture_coverage(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        _write(root / "schemas/agent-spec/v1/agent-spec.schema.json", VALID_SCHEMA)
        _write(
            root
            / "fixtures/compatibility/agent-spec/v1/agent-spec/valid--minimal.json",
            "{}\n",
        )
        self._commit(root, "add v1")
        base = self._head(root)
        src = root / "schemas/agent-spec/v1/agent-spec.schema.json"
        dst = root / "schemas/agent-spec/v2/agent-spec.schema.json"
        dst.parent.mkdir(parents=True, exist_ok=True)
        subprocess.run(
            ["git", "mv", str(src), str(dst)], cwd=root, check=True, capture_output=True
        )
        self._commit(root, "rename v1 to v2 without fixtures")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        subjects = {d.subject for d in diagnostics if d.check_id == "GOV006"}
        assert "agent-spec@v1" in subjects
        assert "agent-spec@v2" in subjects

    def test_wit_path_must_be_versioned(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(root / "plugins/finstack-ai-wit/wit/world.wit", "package demo;\n")
        self._commit(root, "unversioned wit")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (d.check_id == "GOV005" and d.path.endswith(".wit") for d in diagnostics)
        )

    def test_versioned_wit_requires_fixture(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(
            root / "plugins/finstack-ai-wit/wit/v0.1.0/world.wit",
            "package demo:wit@0.1.0;\n",
        )
        self._commit(root, "wit only")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert any(
            (d.check_id == "GOV006" and "wit@v0.1.0" in d.subject for d in diagnostics)
        )

    def test_versioned_wit_with_fixture_passes(self) -> None:
        root = self._git_repo()
        self._tmp_root = root
        base = self._head(root)
        _write(
            root / "plugins/finstack-ai-wit/wit/v0.1.0/world.wit",
            "package demo:wit@0.1.0;\n",
        )
        _write(
            root / "fixtures/compatibility/wit/v0.1.0/world/valid--minimal.wit",
            "package demo:wit@0.1.0;\n",
        )
        self._commit(root, "wit and fixture")
        diagnostics = check_schema_fixture_coupling(root, base_ref=base)
        assert not any((d.check_id == "GOV006" for d in diagnostics))
        assert not any((d.check_id == "GOV005" for d in diagnostics))


class PrTemplateTests:
    def test_missing_impact_sections_fail_gov007(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write(root / ".github/PULL_REQUEST_TEMPLATE.md", "## Summary\n\n-\n")
            diagnostics = check_pr_template(root)
            assert any((d.check_id == "GOV007" for d in diagnostics))

    def test_complete_impact_sections_pass(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            _write(root / ".github/PULL_REQUEST_TEMPLATE.md", _pr_template())
            diagnostics = check_pr_template(root)
            assert not any((d.check_id == "GOV007" for d in diagnostics))


class RunChecksIntegrationTests:
    def test_run_checks_returns_diagnostics_for_empty_repo(self) -> None:
        with tempfile.TemporaryDirectory() as tmp:
            root = Path(tmp)
            diagnostics = run_checks(root)
            assert diagnostics
            assert all((isinstance(d, Diagnostic) for d in diagnostics))

    def test_base_ref_from_environment(self) -> None:
        previous = os.environ.get("SCHEMA_GOVERNANCE_BASE")
        try:
            os.environ["SCHEMA_GOVERNANCE_BASE"] = "HEAD"
            with tempfile.TemporaryDirectory() as tmp:
                root = Path(tmp)
                _seed_registry_paths(root)
                diagnostics = check_schema_fixture_coupling(root, base_ref=None)
                assert any(
                    (d.check_id in {"GOV005", "GOV006"} for d in diagnostics)
                ) or any(("git" in d.message.lower() for d in diagnostics))
        finally:
            if previous is None:
                os.environ.pop("SCHEMA_GOVERNANCE_BASE", None)
            else:
                os.environ["SCHEMA_GOVERNANCE_BASE"] = previous
