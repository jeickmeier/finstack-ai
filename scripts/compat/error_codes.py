#!/usr/bin/env python3
"""Freeze the VALUES of the stable error-code constants.

`public_api.py` freezes that a constant exists and what its type is, but not
what string it holds. Changing `AGENT_RUN_TIMEOUT` from `"agent_run_timeout"`
to anything else therefore passed every gate while breaking the Python and
JavaScript bindings, which match on the value.

Scope is deliberately narrow: constants that are declared today as
`pub const NAME: &str = "...";` and whose value satisfies the kernel's error
code grammar (lowercase `snake_case`, from `error_code_is_valid`). Dotted
identifiers -- tool ids, digest domains, compaction strategy names -- are not
error codes and stay out. Codes returned inline from a `code()` arm without a
named constant are not covered; naming those is separate work, and this
fixture grows when they land.
"""

from __future__ import annotations

import argparse
import json
import re
import sys
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
FIXTURE = REPO_ROOT / "fixtures" / "compatibility" / "error-codes" / "v1" / "codes.json"
ERRORS_TS = REPO_ROOT / "bindings" / "finstack-ai-wasm" / "js" / "src" / "errors.ts"
REGENERATE = "mise run write-public-api"

SCAN_ROOTS = ("crates", "extensions", "plugins", "bindings")
SKIP_PARTS = {"target", "node_modules"}

# `pub const NAME: &str = "value";` including rustfmt-wrapped declarations,
# which an anchored single-line pattern silently misses.
DECL_RE = re.compile(
    r"pub const ([A-Z][A-Z0-9_]*): &(?:'static )?str\s*=\s*\"([^\"]*)\"\s*;",
    re.S,
)
# Mirrors `error_code_is_valid` in
# crates/finstack-ai-kernel/src/primitives/error.rs.
CODE_RE = re.compile(r"^[a-z][a-z0-9]*(?:_[a-z0-9]+)*$")
CODE_MAX_BYTES = 256


def crate_of(path: Path) -> str:
    for parent in path.parents:
        if (parent / "Cargo.toml").is_file():
            return parent.name
    return "<unknown>"


def collect() -> list[dict[str, str]]:
    found: dict[tuple[str, str], dict[str, str]] = {}
    for root in SCAN_ROOTS:
        for path in sorted((REPO_ROOT / root).rglob("*.rs")):
            if any(part in SKIP_PARTS for part in path.parts):
                continue
            if path.name.endswith("tests.rs") or "tests" in path.parts:
                continue
            for name, value in DECL_RE.findall(path.read_text(encoding="utf-8")):
                if not CODE_RE.match(value):
                    continue
                if len(value.encode("utf-8")) > CODE_MAX_BYTES:
                    continue
                crate = crate_of(path)
                found[(crate, name)] = {"crate": crate, "name": name, "value": value}
    return sorted(found.values(), key=lambda e: (e["crate"], e["name"]))


def stable_codes_from_ts() -> list[str]:
    text = ERRORS_TS.read_text(encoding="utf-8")
    block = re.search(r"const STABLE_CODES = \[(.*?)\] as const;", text, re.S)
    if block is None:
        raise SystemExit(f"{ERRORS_TS}: STABLE_CODES array not found")
    return re.findall(r'"([^"]+)"', block.group(1))


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--write", action="store_true")
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if not args.write and not args.check:
        parser.error("pass --write or --check")

    current = collect()
    payload = {"version": 1, "codes": current}

    if args.write:
        FIXTURE.parent.mkdir(parents=True, exist_ok=True)
        FIXTURE.write_text(
            json.dumps(payload, indent=2, sort_keys=False) + "\n", encoding="utf-8"
        )
        return 0

    failed = False
    if not FIXTURE.is_file():
        print(f"missing fixture {FIXTURE}", file=sys.stderr)
        return 1
    frozen = json.loads(FIXTURE.read_text(encoding="utf-8"))["codes"]
    frozen_by_key = {(e["crate"], e["name"]): e["value"] for e in frozen}
    current_by_key = {(e["crate"], e["name"]): e["value"] for e in current}

    for key, value in sorted(frozen_by_key.items()):
        if key not in current_by_key:
            print(
                f"removed error code: {key[0]}::{key[1]} = {value!r}", file=sys.stderr
            )
            failed = True
        elif current_by_key[key] != value:
            print(
                f"changed error code value: {key[0]}::{key[1]}"
                f" {value!r} -> {current_by_key[key]!r}",
                file=sys.stderr,
            )
            failed = True
    for key, value in sorted(current_by_key.items()):
        if key not in frozen_by_key:
            print(f"added error code: {key[0]}::{key[1]} = {value!r}", file=sys.stderr)
            failed = True

    # The TypeScript binding keeps its own copy to classify messages. It must
    # not drift from the Rust values; it may legitimately cover only a subset.
    known = {e["value"] for e in current}
    for code in stable_codes_from_ts():
        if code not in known:
            print(
                f"{ERRORS_TS.relative_to(REPO_ROOT)}: STABLE_CODES entry {code!r}"
                " matches no Rust error-code constant",
                file=sys.stderr,
            )
            failed = True

    if failed:
        print(
            "\nError-code values differ from the frozen fixture in"
            f" {FIXTURE.relative_to(REPO_ROOT)}."
            "\nCode strings are a cross-language contract: Python and"
            " JavaScript match on the value."
            "\nIf the change is intended, regenerate and commit it:"
            f"\n    {REGENERATE}",
            file=sys.stderr,
        )
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
