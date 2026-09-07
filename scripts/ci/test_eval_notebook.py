"""Execute the offline evaluation notebook in the active Python environment."""

from __future__ import annotations

import sys
import tempfile
from pathlib import Path

import nbformat
from jupyter_client import KernelManager
from jupyter_client.kernelspec import KernelSpec
from nbclient import NotebookClient

ROOT = Path(__file__).resolve().parents[2]


def main() -> None:
    notebook = ROOT / "examples/python-notebooks/12_evaluation.ipynb"
    book = nbformat.read(notebook, as_version=4)
    # Use the exact interpreter that imported the freshly built editable extension.
    manager = KernelManager(kernel_name="python3")
    manager._kernel_spec = KernelSpec(
        argv=[sys.executable, "-m", "ipykernel_launcher", "-f", "{connection_file}"],
        display_name="Evaluation CI",
        language="python",
    )
    with tempfile.TemporaryDirectory(prefix="finstack-eval-notebook-") as directory:
        manager.connection_file = str(Path(directory) / "kernel.json")
        client = NotebookClient(
            book,
            km=manager,
            timeout=90,
            resources={"metadata": {"path": str(notebook.parent)}},
        )
        try:
            client.execute()
        finally:
            if manager.has_kernel:
                manager.shutdown_kernel(now=True)
    print("ok: evaluation notebook executed every cell offline")


if __name__ == "__main__":
    main()
