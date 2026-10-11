"""Native link directories for the live libpython.

PyO3 records ``LIBDIR`` in a build-script output that Cargo reuses while
``PYO3_PYTHON`` stays a stable venv or ``python/3.14`` symlink. After mise
or uv moves that symlink to a new patch (3.14.7 to 3.14.8), rust-lld still
searches the old directory and reports ``unable to find library -lpythonX.Y``.

rust-lld also only accepts ``libpythonX.Y.so`` (or ``.dylib`` / ``.a``). A
python-build-standalone tree may ship only ``libpythonX.Y.so.1.0``, or a GNU
ld script of the unversioned name, which rust-lld will not open. Script text
is never used as a path: only filenames derived from the interpreter version,
or ``LDLIBRARY`` when it is itself one of those filenames, are opened.
"""

from __future__ import annotations

import re
import sys
import sysconfig
from pathlib import Path

_VERSION = re.compile(r"\d+\.\d+\Z", re.ASCII)
_LIBRARY_NAME = re.compile(
    r"libpython\d+(?:\.\d+)*(?:\.so(?:\.\d+)*|\.dylib|\.a)\Z",
    re.ASCII,
)


def _version(version: str) -> str | None:
    match = _VERSION.fullmatch(version)
    if match is None:
        return None
    return match.group(0)


def _library_name(name: str) -> str | None:
    match = _LIBRARY_NAME.fullmatch(name)
    if match is None:
        return None
    return match.group(0)


def _library_path(root: Path, name: str) -> Path | None:
    """Return ``name`` inside ``root`` when it is a single library filename."""
    safe_name = _library_name(name)
    if safe_name is None:
        return None
    base = root.expanduser().resolve()
    candidate = base / safe_name
    if candidate.parent != base:
        return None
    return candidate


def linker_names(version: str) -> tuple[str, str, str]:
    """Filenames ``-lpython{version}`` searches for."""
    return (
        f"libpython{version}.so",
        f"libpython{version}.dylib",
        f"libpython{version}.a",
    )


def alias_name(ldlibrary: str, version: str) -> str | None:
    """Unversioned linker filename for a versioned ``LDLIBRARY``."""
    safe = _version(version)
    library = _library_name(ldlibrary)
    if safe is None or library is None:
        return None
    if library.endswith(".dylib"):
        return f"libpython{safe}.dylib"
    if ".so" in library:
        return f"libpython{safe}.so"
    if library.endswith(".a"):
        return f"libpython{safe}.a"
    return None


def is_linker_script(path: Path) -> bool:
    """True when ``path`` is a GNU ld script rather than an object file."""
    candidate = _library_path(path.parent, path.name)
    if candidate is None or not candidate.is_file():
        return False
    try:
        head = candidate.read_bytes()[:64].lstrip()
    except OSError:
        return False
    return head.startswith((b"/*", b"GROUP", b"INPUT", b"OUTPUT_FORMAT"))


def is_linkable(path: Path) -> bool:
    """True when rust-lld can open ``path`` as ``-l`` input."""
    candidate = _library_path(path.parent, path.name)
    if candidate is None:
        return False
    return candidate.is_file() and not is_linker_script(candidate)


def shared_object(libdir: Path, ldlibrary: str, version: str) -> Path | None:
    """Real shared or static library behind ``LDLIBRARY`` or a linker script."""
    safe = _version(version)
    if safe is None:
        return None
    names: list[str] = []
    library = _library_name(ldlibrary)
    if library is not None:
        names.append(library)
    names.extend(
        (
            f"libpython{safe}.so.1.0",
            f"libpython{safe}.so.1",
            f"libpython{safe}.dylib",
            f"libpython{safe}.a",
        )
    )
    seen: set[str] = set()
    for name in names:
        if name in seen:
            continue
        seen.add(name)
        candidate = _library_path(libdir, name)
        if candidate is not None and is_linkable(candidate):
            return candidate
    return None


def ensure_python_link_dir(
    libdir: Path,
    ldlibrary: str,
    version: str,
    dest: Path,
) -> Path | None:
    """Return a directory that must be searched before ``libdir``.

    ``None`` means ``libdir`` already contains a name the linker accepts.
    """
    safe = _version(version)
    if safe is None:
        return None
    for name in linker_names(safe):
        candidate = _library_path(libdir, name)
        if candidate is not None and is_linkable(candidate):
            return None
    real = shared_object(libdir, ldlibrary, safe)
    if real is None:
        return None
    alias = alias_name(real.name, safe)
    if alias is None or real.name == alias:
        return None
    dest.mkdir(parents=True, exist_ok=True)
    link = _library_path(dest, alias)
    if link is None:
        return None
    target = real.resolve()
    if link.is_symlink() and link.resolve() == target:
        return dest
    if link.is_symlink() or link.exists():
        link.unlink()
    link.symlink_to(target)
    return dest


def link_search_dirs(
    libdir: Path,
    ldlibrary: str,
    version: str,
    dest: Path,
) -> list[Path]:
    """Directories rustc must search, alias first, then the live ``LIBDIR``."""
    dirs: list[Path] = []
    alias = ensure_python_link_dir(libdir, ldlibrary, version, dest)
    if alias is not None:
        dirs.append(alias)
    if libdir.as_posix() not in {path.as_posix() for path in dirs}:
        dirs.append(libdir)
    return dirs


def main() -> int:
    libdir_raw = sysconfig.get_config_var("LIBDIR") or ""
    ldlibrary = sysconfig.get_config_var("LDLIBRARY") or ""
    version = f"{sys.version_info.major}.{sys.version_info.minor}"
    if not libdir_raw:
        return 0
    for directory in link_search_dirs(
        Path(libdir_raw),
        str(ldlibrary),
        version,
        Path("target/ci/python-link"),
    ):
        print(directory.resolve())
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
