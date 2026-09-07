"""Execute every offline knowledge notebook with the actual native extension."""

from __future__ import annotations

import argparse
import os
import sys
import tempfile
from pathlib import Path

import nbformat
from jupyter_client import KernelManager
from jupyter_client.kernelspec import KernelSpec
from nbclient import NotebookClient

ROOT = Path(__file__).resolve().parents[2]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--all", action="store_true", help="Run every notebook offline")
    args = parser.parse_args()
    pattern = "*.ipynb" if args.all else "k0[1-5]_*.ipynb"
    for notebook in sorted((ROOT / "examples/python-notebooks").glob(pattern)):
        book = nbformat.read(notebook, as_version=4)
        manager = KernelManager(kernel_name="python3")
        manager._kernel_spec = KernelSpec(
            argv=[
                sys.executable,
                "-m",
                "ipykernel_launcher",
                "-f",
                "{connection_file}",
            ],
            display_name="Knowledge CI",
            language="python",
        )
        with tempfile.TemporaryDirectory(
            prefix="finstack-knowledge-notebook-"
        ) as directory:
            manager.connection_file = str(Path(directory) / "kernel.json")
            client = NotebookClient(
                book,
                km=manager,
                timeout=90,
                resources={"metadata": {"path": str(notebook.parent)}},
            )
            try:
                client.execute(
                    env={
                        **os.environ,
                        "FINSTACK_NOTEBOOK_OFFLINE": "1",
                        "TMPDIR": directory,
                    }
                )
            finally:
                if manager.has_kernel:
                    manager.shutdown_kernel(now=True)
        print(f"ok: {notebook.name} executed every cell offline", flush=True)


if __name__ == "__main__":
    main()
