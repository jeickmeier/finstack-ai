"""Native link directories for the live libpython.

PyO3 records ``LIBDIR`` in a build-script output that Cargo reuses while
``PYO3_PYTHON`` stays a stable venv or ``python/3.14`` symlink. After mise
or uv moves that symlink to a new patch (3.14.7 to 3.14.8), rust-lld still
searches the old directory and reports ``unable to find library -lpythonX.Y``.

rust-lld also only accepts ``libpythonX.Y.so`` (or ``.dylib`` / ``.a``). A
python-build-standalone tree may ship only ``libpythonX.Y.so.1.0``, or a GNU
ld script of the unversioned name, which rust-lld will not open.
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


def is_linker_script(path: Path) -> bool:
    """True when ``path`` is a GNU ld script rather than an object file."""
    if not path.is_file():
        return False
    try:
        head = path.read_bytes()[:64].lstrip()
    except OSError:
        return False
    return head.startswith((b"/*", b"GROUP", b"INPUT", b"OUTPUT_FORMAT"))


def is_linkable(path: Path) -> bool:
    """True when rust-lld can open ``path`` as ``-l`` input."""
    return path.is_file() and not is_linker_script(path)


def _script_member(path: Path) -> str | None:
    try:
        text = path.read_text(encoding="utf-8", errors="replace")
    except OSError:
        return None
    for token in text.replace("(", " ").replace(")", " ").split():
        if token.startswith("libpython"):
            return token
    return None


def shared_object(libdir: Path, ldlibrary: str, version: str) -> Path | None:
    """Real shared or static library behind ``LDLIBRARY`` or a linker script."""
    names: list[str] = []
    script = libdir / f"libpython{version}.so"
    if is_linker_script(script):
        member = _script_member(script)
        if member is not None:
            names.append(member)
    if ldlibrary:
        names.append(ldlibrary)
    names.append(f"libpython{version}.so.1.0")
    seen: set[str] = set()
    for name in names:
        if name in seen:
            continue
        seen.add(name)
        candidate = libdir / name
        if is_linkable(candidate):
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
    if any(is_linkable(libdir / name) for name in linker_names(version)):
        return None
    real = shared_object(libdir, ldlibrary, version)
    if real is None:
        return None
    alias = alias_name(real.name, version)
    if alias is None or real.name == alias:
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
