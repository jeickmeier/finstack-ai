"""Run complete offline Rust and Python lifecycle recipes in bounded processes."""

from __future__ import annotations

import json
import os
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]


def checked(command: list[str], timeout: int, marker: str | None = None) -> str:
    """Run with isolated working storage and preserve failures for CI diagnostics."""
    with tempfile.TemporaryDirectory(prefix="finstack-recipe-gate-") as directory:
        result = subprocess.run(
            command,
            cwd=directory,
            capture_output=True,
            text=True,
            timeout=timeout,
            check=False,
        )
    if result.returncode or (marker is not None and marker not in result.stdout):
        raise AssertionError(f"{command}:\n{result.stdout}\n{result.stderr}")
    print(f"ok: {Path(command[-1]).name}", flush=True)
    return result.stdout


def main() -> None:
    """Run one language per CI job after its runtime has been built."""
    language = sys.argv[1]
    if language == "rust":
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--offline",
                "-p",
                "finstack-ai-knowledge",
                "--example",
                "persistent",
            ],
            cwd=ROOT,
            timeout=600,
            check=True,
        )
        subprocess.run(
            [
                "cargo",
                "build",
                "--locked",
                "--offline",
                "-p",
                "finstack-ai-native-examples",
                "--bin",
                "supervisor",
                "-p",
                "finstack-ai-example-durable-interaction",
            ],
            cwd=ROOT,
            timeout=600,
            check=True,
        )
        metadata = subprocess.run(
            ["cargo", "metadata", "--offline", "--no-deps", "--format-version", "1"],
            cwd=ROOT,
            capture_output=True,
            text=True,
            check=True,
            timeout=30,
        )
        target = Path(json.loads(metadata.stdout)["target_directory"]) / "debug"
        suffix = ".exe" if os.name == "nt" else ""
        for path, marker in [
            ("examples/persistent", "persistent knowledge: disk reopen"),
            ("supervisor", "supervisor: two results"),
            (
                "finstack-ai-example-durable-interaction",
                "resolved and run completed after restart",
            ),
        ]:
            checked([str(target / f"{path}{suffix}")], 60, marker)
    elif language == "python":
        for filename, marker in [
            ("persistent_assistant.py", "persistent knowledge: disk reopen"),
            ("durable_approval.py", "durable approval: fresh process"),
            ("supervisor.py", "supervisor: two results"),
        ]:
            checked(
                [sys.executable, str(ROOT / "examples/python-notebooks" / filename)],
                70,
                marker,
            )
    else:
        raise ValueError("language must be rust or python")


if __name__ == "__main__":
    main()
