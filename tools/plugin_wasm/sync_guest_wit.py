#!/usr/bin/env python3
"""Copy canonical @0.0.4 WIT packages into guest-sdk, templates, and references."""

from __future__ import annotations

import argparse
import shutil
from pathlib import Path

REPO_ROOT = Path(__file__).resolve().parents[2]
CANONICAL = REPO_ROOT / "plugins" / "finstack-ai-wit" / "wit" / "v0.0.4"
PACKAGES = (
    "finstack-ai-types",
    "finstack-ai-host",
    "finstack-ai-toolset",
    "finstack-ai-context",
)
DESTINATIONS = (
    REPO_ROOT / "plugins" / "finstack-ai-guest-sdk" / "wit",
    REPO_ROOT / "plugins" / "templates" / "toolset-plugin" / "wit",
    REPO_ROOT / "plugins" / "templates" / "context-plugin" / "wit",
    REPO_ROOT / "plugins" / "reference" / "calculator" / "wit",
    REPO_ROOT / "plugins" / "reference" / "filesystem-sandbox" / "wit",
    REPO_ROOT / "plugins" / "reference" / "context-provider" / "wit",
    REPO_ROOT
    / "plugins"
    / "finstack-ai-plugin-host"
    / "fixtures"
    / "guests"
    / "echo-toolset"
    / "wit",
)
RESOLVE_WIT = """package finstack:guest-sdk-bindgen@0.0.4;

/// Guest bindgen worlds. Not a published WIT package.
world toolset-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;
    export finstack:ai-toolset/toolset@0.0.4;
}

world context-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;
    export finstack:ai-context/context-provider@0.0.4;
}
"""
WASI_FILES = ("filesystem.wit", "io.wit", "clocks.wit")

SANDBOX_RESOLVE = """package finstack:guest-sdk-bindgen@0.0.4;

/// Guest bindgen worlds. Not a published WIT package.
world toolset-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;
    export finstack:ai-toolset/toolset@0.0.4;
}

world context-plugin {
    import finstack:ai-host/logging@0.0.4;
    import finstack:ai-host/blobs@0.0.4;
    export finstack:ai-context/context-provider@0.0.4;
}

world filesystem-sandbox {
    include toolset-plugin;
    import wasi:filesystem/preopens@0.2.12;
    import wasi:filesystem/types@0.2.12;
}
"""


def package_file(name: str) -> Path:
    stem = name.removeprefix("finstack-ai-")
    return CANONICAL / name / f"{stem}.wit"


def copy_packages(dest: Path) -> None:
    dest.mkdir(parents=True, exist_ok=True)
    deps = dest / "deps"
    deps.mkdir(parents=True, exist_ok=True)
    for name in PACKAGES:
        source = package_file(name)
        target_dir = deps / name
        target_dir.mkdir(parents=True, exist_ok=True)
        shutil.copyfile(source, target_dir / source.name)
    sandbox = dest.parent.name == "filesystem-sandbox"
    resolve = dest / "resolve.wit"
    resolve.write_text(SANDBOX_RESOLVE if sandbox else RESOLVE_WIT, encoding="utf-8")


def check_packages(dest: Path) -> None:
    for name in PACKAGES:
        source = package_file(name)
        copied = dest / "deps" / name / source.name
        if not copied.is_file():
            raise SystemExit(f"missing vendored WIT: {copied}")
        if copied.read_bytes() != source.read_bytes():
            raise SystemExit(f"WIT drift: {copied}")
    expected = (
        SANDBOX_RESOLVE if dest.parent.name == "filesystem-sandbox" else RESOLVE_WIT
    )
    resolve = dest / "resolve.wit"
    if not resolve.is_file():
        raise SystemExit(f"missing resolve.wit: {resolve}")
    if resolve.read_text(encoding="utf-8") != expected:
        raise SystemExit(f"resolve.wit drift: {resolve}")
    if dest.parent.name == "filesystem-sandbox":
        for name in WASI_FILES:
            wasi = dest / "deps" / name
            if not wasi.is_file():
                raise SystemExit(f"missing vendored WASI WIT: {wasi}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    if args.check:
        for dest in DESTINATIONS:
            check_packages(dest)
        return
    for dest in DESTINATIONS:
        copy_packages(dest)


if __name__ == "__main__":
    main()
