"""Linker alias for versioned libpython sonames."""

from __future__ import annotations

import importlib.util
import sys
from pathlib import Path

_SPEC = importlib.util.spec_from_file_location(
    "python_link_dir",
    Path(__file__).resolve().parent / "python_link_dir.py",
)
if _SPEC is None or _SPEC.loader is None:
    raise RuntimeError("python_link_dir module is missing")
_MODULE = importlib.util.module_from_spec(_SPEC)
sys.modules[_SPEC.name] = _MODULE
_SPEC.loader.exec_module(_MODULE)
ensure_python_link_dir = _MODULE.ensure_python_link_dir


def test_versioned_soname_gets_an_unversioned_symlink(tmp_path: Path) -> None:
    libdir = tmp_path / "lib"
    libdir.mkdir()
    soname = libdir / "libpython3.14.so.1.0"
    soname.write_bytes(b"elf")
    dest = tmp_path / "link"
    found = ensure_python_link_dir(libdir, soname.name, "3.14", dest)
    assert found == dest
    link = dest / "libpython3.14.so"
    assert link.is_symlink()
    assert link.resolve() == soname.resolve()


def test_existing_unversioned_library_needs_no_alias(tmp_path: Path) -> None:
    libdir = tmp_path / "lib"
    libdir.mkdir()
    (libdir / "libpython3.14.dylib").write_bytes(b"macho")
    found = ensure_python_link_dir(
        libdir,
        "libpython3.14.dylib",
        "3.14",
        tmp_path / "link",
    )
    assert found is None
    assert not (tmp_path / "link").exists()
