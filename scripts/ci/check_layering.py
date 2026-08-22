"""Enforce the workspace layering law: leaves depend on contracts, never the reverse.

The architecture rules state the tiers in prose (`.agents/rules/01-engineering-
conformance.md`, "Preserve package and semantic boundaries"). Nothing else in CI
can see a violation of them: `cargo deny` inspects licenses, advisories and
version splits, `check-public-api` freezes published surfaces, and the
`fixtures/ci/*-port-leaf` crates prove target correctness. This script closes
that gap by reading the workspace graph directly.

Only `normal` and `build` dependencies are checked. Dev-dependencies are exempt
by design: they never reach a shipped consumer, and the workspace relies on
several deliberately -- `finstack-ai-kernel` dev-depends on
`finstack-ai-protocol` for codec round-trip tests, and most extension leaves
dev-depend on `finstack-ai-test` (and occasionally on the facade) for
conformance fixtures.
"""

from __future__ import annotations

import json
import subprocess
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]

KERNEL = 0
CONTRACTS = 1
EXTENSIONS = 2
COMPOSITION = 3
HOSTS = 4

TIER_NAMES = {
    KERNEL: "kernel",
    CONTRACTS: "contracts",
    EXTENSIONS: "extensions",
    COMPOSITION: "composition",
    HOSTS: "hosts",
}

# Tiers a crate may depend on are strictly below its own, except where
# SAME_TIER_ALLOWED says otherwise. Membership is by manifest directory, so a
# new crate lands in the right tier by where it is placed.
EXACT_TIERS = {
    "crates/finstack-ai-kernel": KERNEL,
    "crates/finstack-ai-runtime": CONTRACTS,
    "crates/finstack-ai-protocol": CONTRACTS,
    "crates/finstack-ai": COMPOSITION,
    "crates/finstack-ai-server": COMPOSITION,
    "crates/finstack-ai-test": COMPOSITION,
}
PREFIX_TIERS = (("extensions/", EXTENSIONS),)
DEFAULT_TIER = HOSTS

# Extension leaves compose with each other (`provider-*` over `provider-wire`,
# `store-*` over `store-common`, networked toolsets over `net-guard`). Host
# packages do too (`plugin-host` over `finstack-ai-wit`). The kernel, the
# contract crates and the composition crates do not: keeping `runtime` and
# `protocol` from naming each other is part of the law.
SAME_TIER_ALLOWED = frozenset({EXTENSIONS, HOSTS})

# Edges the tier table alone would permit but the rules forbid by name.
FORBIDDEN_EDGES = {
    # "runtime and SDK do not depend on protocol for in-process values."
    # Persistent stores and remote/server/client/process adapters are the
    # stated exceptions; the facade itself is not one of them.
    ("finstack-ai", "finstack-ai-protocol"): (
        "the SDK facade must not depend on protocol for in-process values"
    ),
}

# Violations that are known, tracked, and not yet fixed. Each entry must cite
# why it is still here. An entry that no longer corresponds to a real edge is
# itself an error, so this list cannot rot into a permanent exemption.
KNOWN_VIOLATIONS: dict[tuple[str, str], str] = {}


def tier_of(relative_dir: str) -> int:
    if relative_dir in EXACT_TIERS:
        return EXACT_TIERS[relative_dir]
    for prefix, tier in PREFIX_TIERS:
        if relative_dir.startswith(prefix):
            return tier
    return DEFAULT_TIER


def workspace_packages() -> list[dict]:
    completed = subprocess.run(
        ["cargo", "metadata", "--no-deps", "--format-version", "1", "--offline"],
        cwd=REPO_ROOT,
        check=True,
        capture_output=True,
    )
    metadata = json.loads(completed.stdout)
    root = Path(metadata["workspace_root"])
    packages = []
    for package in metadata["packages"]:
        manifest = Path(package["manifest_path"])
        relative_dir = manifest.parent.relative_to(root).as_posix()
        packages.append(
            {
                "name": package["name"],
                "dir": relative_dir,
                "tier": tier_of(relative_dir),
                "dependencies": package["dependencies"],
            }
        )
    return packages


def main() -> int:
    packages = workspace_packages()
    tiers = {package["name"]: package["tier"] for package in packages}
    directories = {package["name"]: package["dir"] for package in packages}

    failures: list[str] = []
    observed_known: set[tuple[str, str]] = set()

    for package in sorted(packages, key=lambda entry: entry["dir"]):
        name = package["name"]
        tier = package["tier"]
        for dependency in package["dependencies"]:
            target = dependency["name"]
            if target not in tiers:
                continue
            if dependency["kind"] not in (None, "build"):
                continue

            edge = (name, target)
            target_tier = tiers[target]
            reason = None

            if edge in FORBIDDEN_EDGES:
                reason = FORBIDDEN_EDGES[edge]
            elif target_tier > tier:
                reason = (
                    f"tier {tier} ({TIER_NAMES[tier]}) must not depend on "
                    f"tier {target_tier} ({TIER_NAMES[target_tier]})"
                )
            elif target_tier == tier and tier not in SAME_TIER_ALLOWED:
                reason = (
                    f"tier {tier} ({TIER_NAMES[tier]}) crates must not depend "
                    "on each other"
                )

            if reason is None:
                continue
            if edge in KNOWN_VIOLATIONS:
                observed_known.add(edge)
                continue
            failures.append(
                f"{directories[name]}/Cargo.toml: {name} -> {target}: {reason}"
            )

    for edge, note in sorted(KNOWN_VIOLATIONS.items()):
        if edge not in observed_known:
            failures.append(
                f"stale KNOWN_VIOLATIONS entry {edge[0]} -> {edge[1]}: the edge "
                "no longer exists, so remove it from scripts/ci/check_layering.py "
                f"({note})"
            )

    if failures:
        print("layering violations found:")
        print("\n".join(f"  {failure}" for failure in failures))
        return 1

    tracked = len(KNOWN_VIOLATIONS)
    suffix = f" ({tracked} tracked violation{'s' if tracked != 1 else ''})"
    print(f"ok: workspace layering holds{suffix if tracked else ''}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
