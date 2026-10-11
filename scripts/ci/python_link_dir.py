"""Expose an unversioned libpython name for rust-lld.

PyO3 links embedding tests with ``-lpythonX.Y``. rust-lld resolves that to
``libpythonX.Y.so`` (or ``.dylib`` / ``.a``). python-build-standalone, which
uv and mise install, often ships only the soname ``libpythonX.Y.so.1.0``.
GNU ld can be pointed at that file by a linker script; rust-lld cannot, and
reports ``unable to find library -lpythonX.Y``.
"""

from __future__ import annotations

import sys
import sysconfig
from pathlib import Path


def linker_names(version: str) -> tuple[str, ...]:
    """Filenames ``-lpython{version}`` searches for."""
    return (
        f"libpython{version}.so",
        f"libpython{version}.dylib",
        f"libpython{version}.a",
    )


def alias_name(ldlibrary: str, version: str) -> str | None:
    """Unversioned linker filename for a versioned ``LDLIBRARY``."""
    if ".dylib" in ldlibrary:
        return f"libpython{version}.dylib"
    if ".so" in ldlibrary:
        return f"libpython{version}.so"
    if ldlibrary.endswith(".a"):
        return f"libpython{version}.a"
    return None


def ensure_python_link_dir(
    libdir: Path,
    ldlibrary: str,
    version: str,
    dest: Path,
) -> Path | None:
    """Return a directory that must be added to the native link path.

    ``None`` means ``libdir`` already contains a name the linker accepts.
    """
    if any((libdir / name).is_file() for name in linker_names(version)):
        return None
    alias = alias_name(ldlibrary, version)
    real = libdir / ldlibrary if ldlibrary else None
    if alias is None or real is None or not real.is_file() or real.name == alias:
        return None
    dest.mkdir(parents=True, exist_ok=True)
    link = dest / alias
    target = real.resolve()
    if link.is_symlink() and link.resolve() == target:
        return dest
    if link.is_symlink() or link.exists():
        link.unlink()
    link.symlink_to(target)
    return dest


def main() -> int:
    libdir_raw = sysconfig.get_config_var("LIBDIR") or ""
    ldlibrary = sysconfig.get_config_var("LDLIBRARY") or ""
    version = f"{sys.version_info.major}.{sys.version_info.minor}"
    if not libdir_raw:
        return 0
    found = ensure_python_link_dir(
        Path(libdir_raw),
        str(ldlibrary),
        version,
        Path("target/ci/python-link"),
    )
    if found is not None:
        print(found.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
