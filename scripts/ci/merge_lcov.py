"""Merge Rust LCOV reports into one SonarQube coverage report.

Native workspace coverage and the kernel-on-WASM probe both record the same
crate paths. SonarQube does not define how overlapping Rust LCOV reports
combine, so CI merges them first: hit counts add, and a line stays covered
when either report covered it. Paths outside the repository are dropped.
"""

from __future__ import annotations

import argparse
import sys
from dataclasses import dataclass, field
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]


@dataclass
class BranchHit:
    taken: int = 0
    executed: bool = False


@dataclass
class FileCoverage:
    lines: dict[int, int] = field(default_factory=dict)
    branches: dict[tuple[int, int, int], BranchHit] = field(default_factory=dict)
    functions: dict[str, int] = field(default_factory=dict)
    function_hits: dict[str, int] = field(default_factory=dict)

    def add_line(self, line: int, hits: int) -> None:
        self.lines[line] = self.lines.get(line, 0) + hits

    def add_branch(self, key: tuple[int, int, int], taken: str) -> None:
        branch = self.branches.setdefault(key, BranchHit())
        if taken == "-":
            return
        branch.executed = True
        branch.taken += int(taken)

    def add_function(self, name: str, line: int, hits: int | None) -> None:
        existing = self.functions.get(name, 0)
        self.functions[name] = existing if existing else line
        if hits is not None:
            self.function_hits[name] = self.function_hits.get(name, 0) + hits


def project_path(raw: str, root: Path) -> str | None:
    path = Path(raw)
    if path.is_absolute():
        try:
            relative = path.resolve().relative_to(root.resolve())
        except ValueError:
            return None
    else:
        relative = Path(*path.parts)
    if relative.is_absolute() or ".." in relative.parts or relative == Path():
        return None
    return relative.as_posix()


def _record(reports: dict[str, FileCoverage], path: str) -> FileCoverage:
    return reports.setdefault(path, FileCoverage())


def parse_lcov(text: str, root: Path) -> tuple[dict[str, FileCoverage], int]:
    reports: dict[str, FileCoverage] = {}
    current: FileCoverage | None = None
    skipped = 0
    for raw_line in text.splitlines():
        line = raw_line.strip()
        if line.startswith("SF:"):
            relative = project_path(line.removeprefix("SF:"), root)
            if relative is None:
                skipped += 1
                current = None
                continue
            current = _record(reports, relative)
        elif current is None or line == "end_of_record":
            continue
        elif line.startswith("DA:"):
            number, hits, *_rest = line.removeprefix("DA:").split(",")
            current.add_line(int(number), int(hits))
        elif line.startswith("BRDA:"):
            number, block, branch, taken = line.removeprefix("BRDA:").split(",")
            current.add_branch((int(number), int(block), int(branch)), taken)
        elif line.startswith("FNDA:"):
            hits, name = line.removeprefix("FNDA:").split(",", 1)
            current.add_function(name, current.functions.get(name, 0), int(hits))
        elif line.startswith("FN:"):
            number, name = line.removeprefix("FN:").split(",", 1)
            current.add_function(name, int(number), None)
    return reports, skipped


def merge_reports(texts: list[str], root: Path) -> str:
    merged: dict[str, FileCoverage] = {}
    skipped = 0
    for text in texts:
        reports, report_skipped = parse_lcov(text, root)
        skipped += report_skipped
        for path, coverage in reports.items():
            target = _record(merged, path)
            for line, hits in coverage.lines.items():
                target.add_line(line, hits)
            for key, branch in coverage.branches.items():
                target.add_branch(key, str(branch.taken) if branch.executed else "-")
            for name, line in coverage.functions.items():
                target.add_function(name, line, None)
            for name, hits in coverage.function_hits.items():
                target.add_function(name, target.functions.get(name, 0), hits)
    if not merged:
        raise SystemExit("LCOV merge produced no repository files")
    rendered = render(merged)
    print(
        f"merged {len(merged)} files, skipped {skipped} paths outside {root}",
        file=sys.stderr,
    )
    return rendered


def render(reports: dict[str, FileCoverage]) -> str:
    chunks: list[str] = []
    for path in sorted(reports):
        coverage = reports[path]
        lines = [f"SF:{path}"]
        for name in sorted(coverage.functions):
            lines.append(f"FN:{coverage.functions[name]},{name}")
        hit_names = sorted(coverage.function_hits)
        for name in hit_names:
            lines.append(f"FNDA:{coverage.function_hits[name]},{name}")
        if coverage.functions:
            covered = sum(
                1
                for name in coverage.functions
                if coverage.function_hits.get(name, 0) > 0
            )
            lines.append(f"FNF:{len(coverage.functions)}")
            lines.append(f"FNH:{covered}")
        for key in sorted(coverage.branches):
            branch = coverage.branches[key]
            taken = str(branch.taken) if branch.executed else "-"
            lines.append(f"BRDA:{key[0]},{key[1]},{key[2]},{taken}")
        if coverage.branches:
            covered_branches = sum(
                1 for branch in coverage.branches.values() if branch.taken > 0
            )
            lines.append(f"BRF:{len(coverage.branches)}")
            lines.append(f"BRH:{covered_branches}")
        for number in sorted(coverage.lines):
            lines.append(f"DA:{number},{coverage.lines[number]}")
        covered_lines = sum(1 for hits in coverage.lines.values() if hits > 0)
        lines.append(f"LH:{covered_lines}")
        lines.append(f"LF:{len(coverage.lines)}")
        lines.append("end_of_record")
        chunks.append("\n".join(lines))
    return "\n".join(chunks) + "\n"


def report_destination(args: argparse.Namespace) -> Path:
    """Return the single path this invocation writes."""
    if args.relativize:
        if args.output is not None or len(args.reports) != 1:
            raise SystemExit(
                "--relativize rewrites exactly one report and takes no --output"
            )
        report = args.reports[0]
        if not isinstance(report, Path):
            raise SystemExit(
                "--relativize rewrites exactly one report and takes no --output"
            )
        return report
    output = args.output
    if not isinstance(output, Path):
        raise SystemExit("--output is required unless --relativize is set")
    return output


def coverage_path(path: Path) -> Path:
    """Resolve ``path`` and rebuild it under ``target/``.

    Callers pass report and output locations on the command line. Rebuilding
    the path from the fixed ``target/`` directory rejects absolute paths and
    ``..`` segments that would read or write outside that directory.
    """
    root = (REPO_ROOT / "target").resolve()
    resolved = Path(path).expanduser().resolve()
    if resolved == root or not resolved.is_relative_to(root):
        raise SystemExit(f"{resolved} is outside {root}")
    return root / resolved.relative_to(root)


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("reports", nargs="+", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument(
        "--relativize",
        action="store_true",
        help="Rewrite one report in place with repository-relative paths",
    )
    parser.add_argument("--root", type=Path, default=REPO_ROOT)
    args = parser.parse_args(argv)
    destination = coverage_path(report_destination(args))
    reports = [coverage_path(path) for path in args.reports]
    root = Path(args.root).expanduser().resolve()
    texts = [path.read_text(encoding="utf-8") for path in reports]
    rendered = merge_reports(texts, root)
    destination.parent.mkdir(parents=True, exist_ok=True)
    destination.write_text(rendered, encoding="utf-8")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
