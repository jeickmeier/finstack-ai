#!/usr/bin/env python3
"""Offline 0.1.0 → 1.0.0 converters for journal, AgentSpec, and WIT manifests."""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
AGENT_SPEC_KEYS = {
    "schema_version",
    "id",
    "model",
    "instructions",
    "toolsets",
    "context_providers",
    "middleware",
    "store",
    "observers",
    "capabilities",
    "limits",
    "policy",
    "extension_config",
}
RECORD_BODY_KINDS = {
    "run_accepted",
    "session_created",
    "lane_created",
    "lane_moved",
    "effect_requested",
    "effect_deferred",
    "effect_completed",
    "effect_failed",
    "effect_cancelled",
    "interaction_requested",
    "interaction_resolved",
    "interaction_expired",
    "interaction_cancelled",
    "entry_appended",
    "context_prepared",
    "retry_scheduled",
    "run_suspended",
    "run_completed",
    "run_failed",
    "run_cancelled",
    "timer_fired",
    "limit_reached",
    "cancellation_requested",
    "cancellation_reconciled",
    "budget_reservation_requested",
    "budget_reservation_released",
    "budget_reservation_settled",
    "budget_charge_recorded",
    "capabilities_activated",
    "output_configured",
    "output_validation_failed",
    "final_result_recorded",
    "tool_batch_opened",
    "tool_batch_closed",
    "tool_call_settled",
    "child_run_prepared",
    "snapshot_written",
    "stage_outcome_recorded",
}
MANIFEST_KEYS = {
    "identity",
    "version",
    "worlds",
    "permissions",
    "configuration_schema",
    "digest",
    "signature",
    "resource_limits",
}
STATE_BEARING_DENY = {"ambient_authority", "unexpected", "secret", "credentials"}


class MigrateError(RuntimeError):
    """Fail-closed conversion failure."""


def raw_json_digest(canonical: bytes) -> str:
    hasher = hashlib.sha256()
    hasher.update(b"finstack-ai")
    hasher.update(b"\x00")
    hasher.update(b"raw-json")
    hasher.update(b"\x00")
    hasher.update((1).to_bytes(4, "big"))
    hasher.update(b"\x00")
    hasher.update(canonical)
    return hasher.hexdigest()


def reject_denied_keys(value: object, path: str) -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            if key in STATE_BEARING_DENY:
                raise MigrateError(f"{path}.{key}: unknown state-bearing field")
            reject_denied_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_denied_keys(child, f"{path}[{index}]")


def manifest_digest(identity: str, version: str, worlds: list[str]) -> str:
    ordered = sorted(set(worlds))
    payload = json.dumps(
        {"identity": identity, "version": version, "worlds": ordered},
        separators=(",", ":"),
        ensure_ascii=False,
    )
    return raw_json_digest(payload.encode("utf-8"))


def convert_manifest(data: dict[str, object]) -> dict[str, object]:
    extra = set(data) - MANIFEST_KEYS
    if extra:
        raise MigrateError(f"unknown manifest field: {sorted(extra)[0]}")
    reject_denied_keys(data, "manifest")
    version = data.get("version")
    if version not in {"0.0.4", "1.0.0"}:
        raise MigrateError("manifest version must be 0.0.4 or 1.0.0")
    identity = data.get("identity")
    worlds = data.get("worlds")
    if not isinstance(identity, str) or not isinstance(worlds, list):
        raise MigrateError("manifest identity/worlds are required")
    world_names = [str(world) for world in worlds]
    converted = dict(data)
    converted["version"] = "1.0.0"
    converted["digest"] = manifest_digest(identity, "1.0.0", world_names)
    return converted


def convert_agent_spec(data: dict[str, object]) -> dict[str, object]:
    extra = set(data) - AGENT_SPEC_KEYS
    if extra:
        raise MigrateError(f"unknown AgentSpec field: {sorted(extra)[0]}")
    reject_denied_keys(data, "agent_spec")
    if data.get("schema_version") != 1:
        raise MigrateError("AgentSpec schema_version must be 1")
    return data


def convert_journal_document(data: dict[str, object]) -> dict[str, object]:
    reject_denied_keys(data, "journal")
    kinds = [key for key in data if key in RECORD_BODY_KINDS]
    if len(kinds) != 1:
        raise MigrateError("journal document must contain exactly one record-body kind")
    extra = set(data) - set(kinds)
    if extra:
        raise MigrateError(f"unknown journal field: {sorted(extra)[0]}")
    return data


def load_json(path: Path) -> object:
    return json.loads(path.read_text(encoding="utf-8"))


def write_json(path: Path, value: object) -> None:
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(
        json.dumps(value, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )


def convert_path(kind: str, source: Path, dest: Path | None) -> object:
    if kind == "journal" and source.suffix == ".jsonl":
        documents = []
        for line_no, line in enumerate(
            source.read_text(encoding="utf-8").splitlines(), start=1
        ):
            if not line.strip():
                continue
            parsed = json.loads(line)
            if not isinstance(parsed, dict):
                raise MigrateError(
                    f"{source}:{line_no}: journal line must be an object"
                )
            documents.append(convert_journal_document(parsed))
        if dest is not None:
            dest.parent.mkdir(parents=True, exist_ok=True)
            dest.write_text(
                "".join(
                    json.dumps(item, separators=(",", ":")) + "\n" for item in documents
                ),
                encoding="utf-8",
            )
        return documents
    data = load_json(source)
    if not isinstance(data, dict):
        raise MigrateError(f"{source}: expected a JSON object")
    converted = {
        "manifest": convert_manifest,
        "agentspec": convert_agent_spec,
        "journal": convert_journal_document,
    }[kind](data)
    if dest is not None:
        write_json(dest, converted)
    return converted


def convert_wit_fixtures(source_dir: Path, dest_dir: Path) -> int:
    count = 0
    for path in sorted(source_dir.rglob("*")):
        if not path.is_file():
            continue
        relative = path.relative_to(source_dir)
        dest = dest_dir / relative
        if path.suffix == ".json" and "manifest" in path.parts:
            data = load_json(path)
            if isinstance(data, dict) and data.get("version") == "0.0.4":
                write_json(dest, convert_manifest(data))
                count += 1
                continue
        dest.parent.mkdir(parents=True, exist_ok=True)
        dest.write_bytes(path.read_bytes())
        count += 1
    return count


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="command", required=True)
    for name in ("manifest", "agentspec", "journal"):
        command = sub.add_parser(name)
        command.add_argument("source", type=Path)
        command.add_argument("--out", type=Path)
        command.add_argument("--dry-run", action="store_true")
    fixtures = sub.add_parser("fixtures")
    fixtures.add_argument(
        "--from",
        dest="source_dir",
        type=Path,
        default=REPO_ROOT / "fixtures" / "compatibility" / "wit" / "v0.0.4",
    )
    fixtures.add_argument(
        "--to",
        dest="dest_dir",
        type=Path,
        default=REPO_ROOT / "fixtures" / "compatibility" / "wit" / "v1.0.0",
    )
    fixtures.add_argument("--dry-run", action="store_true")
    args = parser.parse_args()
    try:
        if args.command == "fixtures":
            if args.dry_run:
                print(f"would convert {args.source_dir} -> {args.dest_dir}")
                return 0
            count = convert_wit_fixtures(args.source_dir, args.dest_dir)
            print(f"converted {count} fixture files")
            return 0
        dest = None if args.dry_run else args.out
        convert_path(args.command, args.source, dest)
        if args.dry_run:
            print(f"{args.command}: ok {args.source}")
        return 0
    except (MigrateError, json.JSONDecodeError, OSError) as error:
        print(error, file=sys.stderr)
        return 1


if __name__ == "__main__":
    raise SystemExit(main())
