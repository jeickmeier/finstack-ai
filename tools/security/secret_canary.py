#!/usr/bin/env python3
"""Prove gitleaks rejects credential-shaped material under examples/ and fixtures/.

Committed repository fixtures store only non-matching fragments. This helper
assembles detector material inside temporary Git repositories so the normal
repository tree never contains credential canaries.
"""

from __future__ import annotations

import json
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
GITLEAKS_CONFIG = REPO_ROOT / ".gitleaks.toml"
FRAGMENT_DIR = REPO_ROOT / "fixtures" / "security" / "canary-redaction"

# Non-secret fragment parts stored in the repository. Assembled only at runtime.
# Matched by the repository-owned `finstack-canary-credential` gitleaks rule.
PREFIX = "finstack_canary_"
BODY_A = "Aa0Bb1Cc2Dd3Ee4Ff5Gg6"
BODY_B = "Hh7Ii8Jj9Kk0Ll1Mm2Nn3Oo"


def assembled_token() -> str:
    """Return the runtime-only credential-shaped token."""
    return f"{PREFIX}{BODY_A}{BODY_B}"


def load_fixture_manifest() -> dict:
    """Load the non-evidentiary redaction fixture metadata."""
    path = FRAGMENT_DIR / "manifest.toml"
    text = path.read_text(encoding="utf-8")
    # Tiny parser for the checked-in TOML-ish key = value pairs we control.
    data: dict[str, object] = {}
    for raw in text.splitlines():
        line = raw.strip()
        if not line or line.startswith("#") or line.startswith("["):
            continue
        key, value = [part.strip() for part in line.split("=", 1)]
        if value in {"true", "false"}:
            data[key] = value == "true"
        elif value.startswith('"') and value.endswith('"'):
            data[key] = value[1:-1]
        else:
            data[key] = value
    return data


def write_canary_file(root: Path, relative: str) -> Path:
    """Write assembled canary content under a temporary repository path."""
    path = root / relative
    path.parent.mkdir(parents=True, exist_ok=True)
    payload = {
        "description": "temporary canary for PR-003 secret-scan negative proof",
        "token": assembled_token(),
    }
    path.write_text(json.dumps(payload, indent=2) + "\n", encoding="utf-8")
    return path


def init_git_repo(root: Path) -> None:
    """Create a temporary git repository with one commit."""
    subprocess.run(["git", "init"], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "config", "user.email", "ci-canary@example.com"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    subprocess.run(
        ["git", "config", "user.name", "CI Canary"],
        cwd=root,
        check=True,
        capture_output=True,
    )
    subprocess.run(["git", "add", "."], cwd=root, check=True, capture_output=True)
    subprocess.run(
        ["git", "commit", "-m", "canary"],
        cwd=root,
        check=True,
        capture_output=True,
    )


def run_gitleaks(source: Path) -> subprocess.CompletedProcess[str]:
    """Run gitleaks against a temporary repository."""
    return subprocess.run(
        [
            "gitleaks",
            "detect",
            "--source",
            str(source),
            "--config",
            str(GITLEAKS_CONFIG),
            "--no-banner",
            "--redact",
            "--verbose",
        ],
        cwd=REPO_ROOT,
        capture_output=True,
        text=True,
        check=False,
    )


def assert_detection(label: str, relative_path: str) -> None:
    """Fail unless gitleaks detects the temporary canary."""
    with tempfile.TemporaryDirectory(prefix=f"finstack-canary-{label}-") as tmp:
        root = Path(tmp)
        write_canary_file(root, relative_path)
        # Copy the repository gitleaks config into the temp tree so relative
        # allowlists, if any, are evaluated against this synthetic source.
        shutil.copy2(GITLEAKS_CONFIG, root / ".gitleaks.toml")
        init_git_repo(root)
        result = run_gitleaks(root)
        output = (result.stdout or "") + (result.stderr or "")
        print(f"=== canary path: {relative_path} ===", flush=True)
        print(output, flush=True)
        if result.returncode == 0:
            raise SystemExit(
                f"expected gitleaks to fail for temporary {relative_path}, "
                f"but it exited 0"
            )
        if result.returncode not in {1}:
            raise SystemExit(
                f"gitleaks returned unexpected exit {result.returncode} for "
                f"{relative_path}"
            )
        if "finstack-canary-credential" not in output:
            raise SystemExit(
                f"expected finstack-canary-credential rule hit for {relative_path}"
            )


def main() -> int:
    """Entrypoint for `mise run secret-scan-canary`."""
    print(
        "controls: Engineering Standards §8; SEC-INV-005; "
        "Threat Model TM-04; Threat Model §§10,13.1",
        flush=True,
    )
    manifest = load_fixture_manifest()
    if manifest.get("evidence_eligible") is not False:
        raise SystemExit("canary-redaction fixture must set evidence_eligible = false")
    if manifest.get("contract_only_not_executed") is not True:
        raise SystemExit(
            "canary-redaction fixture must set contract_only_not_executed = true"
        )

    # Prove both application and fixture trees are covered without allowlists.
    assert_detection("examples", "examples/leaked-token.json")
    assert_detection("fixtures", "fixtures/leaked-token.json")
    print("secret-scan-canary: temporary examples/ and fixtures/ detections passed")
    return 0


if __name__ == "__main__":
    sys.exit(main())
