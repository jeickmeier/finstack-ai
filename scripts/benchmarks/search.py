"""Run the native deterministic benchmark with per-process peak RSS and host metadata.

Build first through ``mise run bench-search``. An explicit empty --corpus-dir retains
sources for a subsequent --reuse measurement of the same data after a code change.
"""

from __future__ import annotations

import argparse
import datetime
import json
import platform
import subprocess
import sys
import tempfile
from pathlib import Path


def measured(binary: str, arguments: list[str]) -> dict:
    import resource  # Unix peak RSS; no guessed zero on an unsupported platform.

    completed = subprocess.run(
        [binary, *arguments],
        check=True,
        text=True,
        stdout=subprocess.PIPE,
        timeout=3600,
    )
    result = json.loads(completed.stdout)
    rss = resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss
    result["peak_rss_bytes"] = rss if sys.platform == "darwin" else rss * 1024
    return result


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", default="target/release-fast/examples/search_scale")
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
    binary = str(Path(args.binary).resolve())
    with tempfile.TemporaryDirectory(prefix="finstack-search-bench-") as temporary:
        root = args.corpus_dir or Path(temporary)
        command = [
            "--root",
            str(root),
            "--chunks",
            str(args.chunks),
            "--memories",
            str(args.memories),
            "--journal-entries",
            str(args.journal_entries),
            "--samples",
            str(args.samples),
        ]

        # Separate measurement processes keep each RSS high-water mark independent.
        def run(extra: list[str]) -> dict:
            completed = subprocess.run(
                [sys.executable, __file__, "_measure", binary, *command, *extra],
                check=True,
                text=True,
                stdout=subprocess.PIPE,
                timeout=3700,
            )
            return json.loads(completed.stdout)

        full = run(["--reuse"] if args.reuse else [])
        memory = run(["--reuse", "--memory-only"])
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
            f"Search benchmark: {args.output}; peak RSS {full['peak_rss_bytes']:,} bytes, memory-only {memory['peak_rss_bytes']:,} bytes"
        )


if __name__ == "__main__":
    if len(sys.argv) > 1 and sys.argv[1] == "_measure":
        print(json.dumps(measured(sys.argv[2], sys.argv[3:])))
    else:
        main()
