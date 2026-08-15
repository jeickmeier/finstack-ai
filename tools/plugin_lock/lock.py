#!/usr/bin/env python3
"""Write or check the local plugin lockfile from on-disk components.

This installer hashes local files only. It does not import the plugin host,
run cargo, or open a network socket.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import sys
import tempfile
from pathlib import Path
from typing import Final

REPO_ROOT: Final[Path] = Path(__file__).resolve().parents[2]
DEFAULT_LOCK: Final[Path] = REPO_ROOT / "plugins" / "reference" / "plugin.lock.json"
DEFAULT_NAMES: Final[tuple[str, ...]] = (
    "calculator",
    "context-provider",
    "filesystem-sandbox",
)


def sha256_hex(path: Path) -> str:
    """Return the hex SHA-256 of a local file."""
    digest = hashlib.sha256()
    digest.update(path.read_bytes())
    return digest.hexdigest()


def default_enabled(name: str) -> bool:
    """Return whether a reference directory is enabled by default."""
    return name != "filesystem-sandbox"


def entry_for(directory: Path, enabled: bool) -> dict[str, object]:
    """Build one lock entry from a local component directory.

    Args:
        directory: Directory containing ``component.wasm`` and
            ``plugin.manifest.json``.
        enabled: Whether runtime ``load_enabled`` should load the entry.

    Returns:
        A lock-entry object. ``manifest_digest`` is copied from the
        on-disk manifest ``digest`` field.

    Raises:
        FileNotFoundError: A required file is missing.
        KeyError: The manifest omits identity, version, or digest.
    """
    manifest_path = directory / "plugin.manifest.json"
    component_path = directory / "component.wasm"
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    name = directory.name
    return {
        "component": f"{name}/component.wasm",
        "component_digest": sha256_hex(component_path),
        "enabled": enabled,
        "identity": manifest["identity"],
        "manifest": f"{name}/plugin.manifest.json",
        "manifest_digest": manifest["digest"],
        "version": manifest["version"],
    }


def render(plugins: list[dict[str, object]]) -> str:
    """Serialize a lock document with stable key order."""
    document = {"lockfile_version": 1, "plugins": plugins}
    return json.dumps(document, indent=2, sort_keys=True) + "\n"


def generate(directories: list[Path]) -> str:
    """Render a lockfile for ``directories``."""
    plugins = [
        entry_for(directory, default_enabled(directory.name))
        for directory in directories
    ]
    return render(plugins)


def default_directories() -> list[Path]:
    """Return the three published reference directories."""
    root = REPO_ROOT / "plugins" / "reference"
    return [root / name for name in DEFAULT_NAMES]


def write_lock(path: Path, body: str) -> None:
    """Write ``body`` to ``path``."""
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(body, encoding="utf-8")


def check_lock(path: Path, body: str) -> int:
    """Return 0 when ``path`` is byte-identical to ``body``."""
    current = path.read_bytes() if path.is_file() else b""
    expected = body.encode("utf-8")
    if current == expected:
        return 0
    with tempfile.NamedTemporaryFile(
        mode="w",
        encoding="utf-8",
        suffix=".plugin.lock.json",
        delete=False,
    ) as handle:
        handle.write(body)
        generated = handle.name
    print(
        f"plugin lockfile drift: {path} does not match regenerated {generated}",
        file=sys.stderr,
    )
    return 1


def main(argv: list[str] | None = None) -> int:
    """Generate or check ``plugins/reference/plugin.lock.json``.

    Args:
        argv: Optional argument list. Defaults to ``sys.argv[1:]``.

    Returns:
        Process exit status.
    """
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--check",
        action="store_true",
        help="regenerate into a temp comparison and require a byte-identical lock",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=DEFAULT_LOCK,
        help="lockfile path (default: plugins/reference/plugin.lock.json)",
    )
    parser.add_argument(
        "directories",
        nargs="*",
        type=Path,
        help="local component directories (default: the three plugins/reference/* dirs)",
    )
    args = parser.parse_args(argv)
    directories = args.directories or default_directories()
    body = generate(directories)
    if args.check:
        return check_lock(args.output, body)
    write_lock(args.output, body)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
