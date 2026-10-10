"""Run the native deterministic benchmark with per-process peak RSS and host metadata.

Build first through ``mise run bench-search``. An explicit empty --corpus-dir retains
sources for a subsequent --reuse measurement of the same data after a code change.
"""

from __future__ import annotations

import argparse
import datetime
import json
import platform
import re
import subprocess
import sys
import tempfile
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
TARGET_DIR = REPO_ROOT / "target"
_BINARY_PART = re.compile(r"[A-Za-z0-9._-]+")
_BINARY_ERROR = (
    "benchmark binary must be an existing file named search_scale under target/"
)


def approved_binary(candidate: Path, target_root: Path = TARGET_DIR) -> Path:
    """Return a search_scale executable rebuilt under ``target_root``.

    The relative path is restricted to a single path segment class so a
    caller-supplied path cannot escape the benchmark output directory.
    """
    resolved = Path(candidate).expanduser().resolve()
    root = Path(target_root).expanduser().resolve()
    if not resolved.is_file():
        raise SystemExit(_BINARY_ERROR)
    try:
        relative = resolved.relative_to(root)
    except ValueError as error:
        raise SystemExit(_BINARY_ERROR) from error
    if relative.name != "search_scale" or any(
        part in {".", ".."} or _BINARY_PART.fullmatch(part) is None
        for part in relative.parts
    ):
        raise SystemExit(_BINARY_ERROR)
    approved = root.joinpath(*relative.parts)
    if not approved.is_file():
        raise SystemExit(_BINARY_ERROR)
    return approved


def benchmark_argv(
    binary: Path,
    root: Path,
    chunks: int,
    memories: int,
    journal_entries: int,
    samples: int,
    *,
    reuse: bool,
    memory_only: bool,
    target_root: Path = TARGET_DIR,
) -> list[str]:
    """Build the native search_scale argument vector from typed fields."""
    argv = [
        str(approved_binary(binary, target_root)),
        "--root",
        str(Path(root).expanduser().resolve()),
        "--chunks",
        str(chunks),
        "--memories",
        str(memories),
        "--journal-entries",
        str(journal_entries),
        "--samples",
        str(samples),
    ]
    if reuse:
        argv.append("--reuse")
    if memory_only:
        argv.append("--memory-only")
    return argv


def measured(
    binary: Path,
    root: Path,
    chunks: int,
    memories: int,
    journal_entries: int,
    samples: int,
    *,
    reuse: bool,
    memory_only: bool,
) -> dict[str, object]:
    import resource  # Unix peak RSS; no guessed zero on an unsupported platform.

    completed = subprocess.run(
        benchmark_argv(
            binary,
            root,
            chunks,
            memories,
            journal_entries,
            samples,
            reuse=reuse,
            memory_only=memory_only,
        ),
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        timeout=3600,
    )
    result = json.loads(completed.stdout)
    if not isinstance(result, dict):
        raise SystemExit("search_scale did not return a JSON object")
    rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    result["peak_rss_bytes"] = rss if sys.platform == "darwin" else rss * 1024
    return result


def _measure_parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description="Measure one search_scale process.")
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--root", type=Path, required=True)
    parser.add_argument("--chunks", type=int, required=True)
    parser.add_argument("--memories", type=int, required=True)
    parser.add_argument("--journal-entries", type=int, required=True)
    parser.add_argument("--samples", type=int, required=True)
    parser.add_argument("--reuse", action="store_true")
    parser.add_argument("--memory-only", action="store_true")
    return parser


def measure_main(argv: list[str]) -> dict[str, object]:
    args = _measure_parser().parse_args(argv)
    return measured(
        args.binary,
        args.root,
        args.chunks,
        args.memories,
        args.journal_entries,
        args.samples,
        reuse=args.reuse,
        memory_only=args.memory_only,
    )


def _child_command(
    binary: Path,
    root: Path,
    chunks: int,
    memories: int,
    journal_entries: int,
    samples: int,
    *,
    reuse: bool,
    memory_only: bool,
) -> list[str]:
    command = [
        sys.executable,
        __file__,
        "_measure",
        "--binary",
        str(binary),
        "--root",
        str(root),
        "--chunks",
        str(chunks),
        "--memories",
        str(memories),
        "--journal-entries",
        str(journal_entries),
        "--samples",
        str(samples),
    ]
    if reuse:
        command.append("--reuse")
    if memory_only:
        command.append("--memory-only")
    return command


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--binary",
        type=Path,
        default=Path("target/release-fast/examples/search_scale"),
    )
    parser.add_argument(
        "--output", type=Path, default=Path("target/search-benchmark.json")
    )
    parser.add_argument("--corpus-dir", type=Path)
    parser.add_argument("--reuse", action="store_true")
    parser.add_argument("--chunks", type=int, default=100_000)
    parser.add_argument("--memories", type=int, default=10_000)
    parser.add_argument("--journal-entries", type=int, default=1000)
    parser.add_argument("--samples", type=int, default=30)
    args = parser.parse_args()
    if args.reuse and args.corpus_dir is None:
        parser.error("--reuse requires --corpus-dir")
    binary = approved_binary(args.binary)
    with tempfile.TemporaryDirectory(prefix="finstack-search-bench-") as temporary:
        root = args.corpus_dir or Path(temporary)

        # Separate measurement processes keep each RSS high-water mark independent.
        def run(*, reuse: bool, memory_only: bool) -> dict[str, object]:
            completed = subprocess.run(
                _child_command(
                    binary,
                    root,
                    args.chunks,
                    args.memories,
                    args.journal_entries,
                    args.samples,
                    reuse=reuse,
                    memory_only=memory_only,
                ),
                check=True,
                text=True,
                stdout=subprocess.PIPE,
                timeout=3700,
            )
            parsed = json.loads(completed.stdout)
            if not isinstance(parsed, dict):
                raise SystemExit("measurement process returned a non-object")
            return parsed

        full = run(reuse=args.reuse, memory_only=False)
        memory = run(reuse=True, memory_only=True)
        output = {
            "recorded_at": datetime.datetime.now(datetime.UTC).isoformat(),
            "host": {
                "system": platform.system(),
                "release": platform.release(),
                "machine": platform.machine(),
                "python": platform.python_version(),
            },
            "profile": "release-fast (opt-level=3, no LTO, 16 codegen units)",
            "full": full,
            "memory_isolated": memory,
        }
        args.output.parent.mkdir(parents=True, exist_ok=True)
        args.output.write_text(json.dumps(output, indent=2) + "\n")
        print(
            "Search benchmark: "
            f"{args.output}; peak RSS {full['peak_rss_bytes']:,} bytes, "
            f"memory-only {memory['peak_rss_bytes']:,} bytes"
        )


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "_measure":
        print(json.dumps(measure_main(sys.argv[2:])))
    else:
        main()
