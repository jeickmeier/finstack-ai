"""Execute the documented offline Rust starters, including their real tool loops."""

from __future__ import annotations

import json
import os
import subprocess
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
EXAMPLES = {
    "minimal": ("minimal ready",),
    "coding": ("filesystem tools: 6", "shell tools: 1", "coding result: five"),
    "service": ("service ready",),
    "diagnostic": ("agent_spec=", "lock_fingerprint=", "lock="),
    "finstack-ai-example-durable-interaction": (
        "resolved and run completed after restart",
        "abandoned interaction reconciled to Closed",
    ),
}


def _build_examples() -> None:
    subprocess.run(
        [
            "cargo",
            "build",
            "--offline",
            "--locked",
            "--bins",
            "-p",
            "finstack-ai-native-examples",
            "-p",
            "finstack-ai-example-durable-interaction",
        ],
        cwd=ROOT,
        check=True,
        timeout=600,
    )


def _debug_dir() -> Path:
    metadata = subprocess.run(
        ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
        cwd=ROOT,
        check=True,
        capture_output=True,
        text=True,
        timeout=30,
    )
    return Path(json.loads(metadata.stdout)["target_directory"]) / "debug"


def _run_example(executable: Path) -> subprocess.CompletedProcess[str]:
    with tempfile.TemporaryDirectory(prefix="finstack-example-") as directory:
        return subprocess.run(
            [str(executable)],
            cwd=directory,
            check=False,
            capture_output=True,
            text=True,
            timeout=60,
        )


def _check_diagnostic_json(stdout: str) -> None:
    for line in stdout.splitlines():
        if line.startswith(("agent_spec=", "lock=")):
            json.loads(line.split("=", 1)[1])


def _check_example(binary: str, expected: tuple[str, ...], executable: Path) -> None:
    result = _run_example(executable)
    if result.returncode:
        raise AssertionError(f"{binary} failed:\n{result.stdout}\n{result.stderr}")
    for marker in expected:
        if marker not in result.stdout:
            raise AssertionError(f"{binary}: missing {marker!r}: {result.stdout}")
    if binary == "diagnostic":
        _check_diagnostic_json(result.stdout)


def main() -> None:
    _build_examples()
    target = _debug_dir()
    suffix = ".exe" if os.name == "nt" else ""
    for binary, expected in EXAMPLES.items():
        _check_example(binary, expected, target / f"{binary}{suffix}")
        print(f"ok: {binary}", flush=True)


if __name__ == "__main__":
    main()
