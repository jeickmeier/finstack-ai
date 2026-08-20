#!/usr/bin/env python3
"""Execute the Python learning notebooks from the repository root."""

from __future__ import annotations

import json
import os
import sys
import tempfile
import threading
from http.server import BaseHTTPRequestHandler, HTTPServer
from pathlib import Path

from nbclient import NotebookClient
from nbformat import read

REPO_ROOT = Path(__file__).resolve().parents[2]
NOTEBOOKS = REPO_ROOT / "examples" / "python-notebooks"
SUPPORT = NOTEBOOKS / "_support.py"
EXPECTED = (
    "01_orientation.ipynb",
    "02_first_agent.ipynb",
    "03_tools_and_structured_output.ipynb",
    "04_runs_events_sessions.ipynb",
    "05_ollama_and_harness.ipynb",
    "06_openai.ipynb",
    "07_anthropic.ipynb",
    "08_document_ingestion.ipynb",
    "09_openrouter.ipynb",
    "10_elicitation.ipynb",
    "11_memory.ipynb",
)
SECRET_MARKERS = (
    "sk-",
    "Bearer ",
    "x-api-key",
    "OPENAI_API_KEY=",
    "ANTHROPIC_API_KEY=",
    "OPENROUTER_API_KEY=",
)


def _outputs(cell: object) -> list[object]:
    outputs = getattr(cell, "outputs", None)
    if not outputs:
        return []
    return list(outputs)


def _output_text(outputs: list[object]) -> str:
    chunks: list[str] = []
    for output in outputs:
        if isinstance(output, dict):
            text = output.get("text") or output.get("evalue") or ""
            if isinstance(text, list):
                chunks.append("".join(str(part) for part in text))
            else:
                chunks.append(str(text))
            data = output.get("data") or {}
            if isinstance(data, dict):
                for value in data.values():
                    if isinstance(value, list):
                        chunks.append("".join(str(part) for part in value))
                    else:
                        chunks.append(str(value))
            continue
        text = getattr(output, "text", None)
        if text:
            chunks.append("".join(text) if isinstance(text, list) else str(text))
        evalue = getattr(output, "evalue", None)
        if evalue:
            chunks.append(str(evalue))
        data = getattr(output, "data", None)
        if isinstance(data, dict):
            for value in data.values():
                chunks.append("".join(value) if isinstance(value, list) else str(value))
    return "\n".join(chunks)


def _reject_secrets(label: str, text: str) -> None:
    for marker in SECRET_MARKERS:
        if marker in text:
            raise SystemExit(f"{label} contains secret-shaped text: {marker!r}")


def _check_committed(path: Path) -> None:
    notebook = json.loads(path.read_text(encoding="utf-8"))
    for index, cell in enumerate(notebook.get("cells", [])):
        outputs = cell.get("outputs") or []
        if outputs:
            raise SystemExit(f"{path.name} cell {index} has stored outputs")
        _reject_secrets(
            f"{path.name} cell {index} stored output",
            json.dumps(outputs),
        )
        if cell.get("cell_type") != "code":
            continue
        source = cell.get("source", "")
        if isinstance(source, list):
            source = "".join(source)
        _reject_secrets(f"{path.name} cell {index} source", source)
        if (
            (
                "https://api.openai.com" in source
                or "https://api.anthropic.com" in source
                or "https://openrouter.ai" in source
            )
            and "live(" not in source
            and "live_value(" not in source
        ):
            raise SystemExit(
                f"{path.name} cell {index} calls a live provider without live()"
            )


def _write_kernelspec(directory: Path) -> str:
    name = "finstack-notebooks"
    spec_dir = directory / "kernels" / name
    spec_dir.mkdir(parents=True)
    (spec_dir / "kernel.json").write_text(
        json.dumps(
            {
                "argv": [
                    sys.executable,
                    "-m",
                    "ipykernel_launcher",
                    "-f",
                    "{connection_file}",
                ],
                "display_name": "finstack-notebooks",
                "language": "python",
            }
        ),
        encoding="utf-8",
    )
    return name


def _check_support_helpers() -> None:
    sys.path.insert(0, str(NOTEBOOKS))
    import _support

    os.environ["NOTEBOOK_HELPER_EMPTY"] = ""
    if _support.live():
        raise SystemExit("live() must be false when no names are given")
    if _support.live("NOTEBOOK_HELPER_EMPTY"):
        raise SystemExit("live() must be false when a named variable is empty")
    if _support.live("NOTEBOOK_HELPER_MISSING"):
        raise SystemExit("live() must be false when a named variable is unset")
    os.environ["NOTEBOOK_HELPER_PRESENT"] = "present"
    if not _support.live("NOTEBOOK_HELPER_PRESENT"):
        raise SystemExit("live() must be true when named variables are set")
    if _support.secret("NOTEBOOK_HELPER_PRESENT") != "present":
        raise SystemExit("secret() must return the named value")
    if _support.live_value("") is not None:
        raise SystemExit("live_value() must be None when explicit and env are empty")
    if _support.live_value("", "NOTEBOOK_HELPER_EMPTY") is not None:
        raise SystemExit("live_value() must be None when the named variable is empty")
    if _support.live_value("", "NOTEBOOK_HELPER_MISSING") is not None:
        raise SystemExit("live_value() must be None when the named variable is unset")
    if _support.live_value("notebook-key") != "notebook-key":
        raise SystemExit("live_value() must return the explicit value")
    if _support.live_value("", "NOTEBOOK_HELPER_PRESENT") != "present":
        raise SystemExit("live_value() must fall back to the named env value")
    if _support.live_value("notebook-key", "NOTEBOOK_HELPER_PRESENT") != "notebook-key":
        raise SystemExit("live_value() must prefer the explicit value")
    try:
        _support.secret("NOTEBOOK_HELPER_MISSING")
    except KeyError as error:
        if "present" in str(error):
            raise SystemExit("secret() leaked a different value") from error
    else:
        raise SystemExit("secret() must raise KeyError when unset")
    if _support.ollama_live("http://127.0.0.1:9") is not None:
        raise SystemExit("ollama_live() must be None when the server is unreachable")
    if _support.ollama_installed("http://127.0.0.1:9") is not None:
        raise SystemExit(
            "ollama_installed() must be None when the server is unreachable"
        )
    if _support.ollama_preferred_model() != "gemma4:26b":
        raise SystemExit("ollama_preferred_model() must default to gemma4:26b")
    _check_ollama_model_selection(_support)
    os.environ.pop("NOTEBOOK_HELPER_EMPTY", None)
    os.environ.pop("NOTEBOOK_HELPER_PRESENT", None)


def _check_ollama_model_selection(support: object) -> None:
    payload = json.dumps(
        {
            "models": [
                {
                    "name": "gpt-oss:20b",
                    "capabilities": ["completion", "tools"],
                },
                {
                    "name": "gemma4:26b",
                    "capabilities": ["completion", "tools"],
                },
            ]
        }
    ).encode()

    class Handler(BaseHTTPRequestHandler):
        def do_GET(self) -> None:
            self.send_response(200)
            self.send_header("Content-Type", "application/json")
            self.send_header("Content-Length", str(len(payload)))
            self.end_headers()
            self.wfile.write(payload)

        def log_message(self, format: str, *args: object) -> None:
            del format, args

    server = HTTPServer(("127.0.0.1", 0), Handler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    url = f"http://127.0.0.1:{server.server_address[1]}"
    previous = os.environ.pop("OLLAMA_MODEL", None)
    try:
        if support.ollama_live(url) != "gemma4:26b":
            raise SystemExit(
                "ollama_live() must select gemma4:26b when it is installed"
            )
        os.environ["OLLAMA_MODEL"] = "gpt-oss:20b"
        if support.ollama_live(url) != "gpt-oss:20b":
            raise SystemExit("ollama_live() must honor OLLAMA_MODEL")
        os.environ["OLLAMA_MODEL"] = "missing-model"
        if support.ollama_live(url) is not None:
            raise SystemExit(
                "ollama_live() must be None when the preferred model is missing"
            )
        if support.ollama_installed(url) != ["gpt-oss:20b", "gemma4:26b"]:
            raise SystemExit(
                "ollama_installed() must list names without preferring tools models"
            )
    finally:
        if previous is None:
            os.environ.pop("OLLAMA_MODEL", None)
        else:
            os.environ["OLLAMA_MODEL"] = previous
        server.shutdown()
        server.server_close()


def _execute(path: Path, kernel_name: str) -> None:
    notebook = read(path, as_version=4)
    client = NotebookClient(
        notebook,
        timeout=120,
        kernel_name=kernel_name,
        resources={"metadata": {"path": str(NOTEBOOKS)}},
    )
    client.execute()
    for index, cell in enumerate(notebook.cells):
        text = _output_text(_outputs(cell))
        _reject_secrets(f"{path.name} executed cell {index}", text)


def main() -> int:
    if not SUPPORT.is_file():
        raise SystemExit(f"missing {SUPPORT.relative_to(REPO_ROOT)}")
    found = tuple(sorted(path.name for path in NOTEBOOKS.glob("*.ipynb")))
    if found != EXPECTED:
        raise SystemExit(f"notebook set mismatch: {found} != {EXPECTED}")
    _check_support_helpers()
    for name in EXPECTED:
        _check_committed(NOTEBOOKS / name)
    with tempfile.TemporaryDirectory() as raw:
        kernel_root = Path(raw)
        kernel_name = _write_kernelspec(kernel_root)
        os.environ["JUPYTER_PATH"] = str(kernel_root.parent)
        os.environ["JUPYTER_DATA_DIR"] = str(kernel_root)
        for name in EXPECTED:
            print(f"+ notebook {name}", flush=True)
            _execute(NOTEBOOKS / name, kernel_name)
    print("ok python learning notebooks")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
