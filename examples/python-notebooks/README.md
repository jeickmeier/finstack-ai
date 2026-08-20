# Python learning notebooks

Nine notebooks that teach `finstack_ai.Agent` as the composition root.
There is no Python `Harness` type. A harness is the recipe: pick a
provider factory, attach trusted Python ports, run, and inspect events.

Workspace pin is `finstack-ai==1.0.0` (unpublished; last public tag
`v0.1.0`; not on PyPI).

## Quick start

From the repository root, sync the workspace environment. That
environment installs editable `finstack-ai[pydantic]`, Jupyter, and
`ipykernel` into `.venv`.

```bash
uv sync
```

If you have pulled new Rust changes since the extension module was last
built (symptom: `ImportError: cannot import name ... from
'finstack_ai._finstack_ai'`), rebuild the native module in place before
running any notebook:

```bash
PYO3_PYTHON="$(uv python find 3.14)" uv run --no-project --with maturin==1.14.1 \
  maturin develop --locked --manifest-path bindings/finstack-ai-python/Cargo.toml \
  --features extension-module
```

Register the kernel once, then select **finstack-ai-notebooks** in Jupyter
or Cursor (the kernel uses the repository-root `.venv/bin/python`):

```bash
uv run python -m ipykernel install --user --name=finstack-ai-notebooks \
  --display-name="finstack-ai-notebooks"
```

Or launch Jupyter from that environment:

```bash
uv run jupyter notebook examples/python-minimal/notebooks
```

Verification (`uv run python scripts/docs/notebooks.py` from the repository root):

```bash
uv run python scripts/docs/notebooks.py
```

## Trust and network

| Notebook | Trust | Network |
| --- | --- | --- |
| [01_orientation.ipynb](01_orientation.ipynb) | Import only | None |
| [02_first_agent.ipynb](02_first_agent.ipynb) | [T2](../../../docs/site/security-trust-levels.md) callback | None |
| [03_tools_and_structured_output.ipynb](03_tools_and_structured_output.ipynb) | T2 callback + Pydantic extra | None |
| [04_runs_events_sessions.ipynb](04_runs_events_sessions.ipynb) | T2 callback | None |
| [05_ollama_and_harness.ipynb](05_ollama_and_harness.ipynb) | T1 provider + T2 ports | Construct-only unless local Ollama has `gemma4:26b` |
| [06_openai.ipynb](06_openai.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `OPENAI_API_KEY`. Set `OPENAI_MODEL`, `OPENAI_REASONING_EFFORT`, and `OPENAI_REASONING_SUMMARY` in the first code cell |
| [07_anthropic.ipynb](07_anthropic.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `ANTHROPIC_API_KEY` |
| [08_document_ingestion.ipynb](08_document_ingestion.ipynb) | T2 callback | None |
| [09_openrouter.ipynb](09_openrouter.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `OPENROUTER_API_KEY`. Set `OPENROUTER_MODEL`, `OPENROUTER_REFERER`, `OPENROUTER_TITLE`, `OPENROUTER_REASONING_EFFORT`, and `OPENROUTER_REASONING_SUMMARY` in the first code cell |

T1 native providers and T2 Python callbacks run in-process. They are not
isolated. See [trust levels](../../../docs/site/security-trust-levels.md).

## Live cells

Notebooks 01–04 and 08 stay offline. 05 constructs `Agent.ollama` with
`gemma4:26b` (or `OLLAMA_MODEL`) and runs live when that model is
installed at `http://127.0.0.1:11434`; otherwise the live cell skips
and lists installed models. 06, 07, and 09 construct offline and run
live when the first code cell or `OPENAI_API_KEY` / `ANTHROPIC_API_KEY`
/ `OPENROUTER_API_KEY` is set. Notebook 06 also reads `OPENAI_MODEL` and
`OPENAI_REASONING_EFFORT` from that first cell; notebook 09 also reads
`OPENROUTER_MODEL` and `OPENROUTER_REASONING_EFFORT`.

`finstack_ai` does not read environment variables. Notebooks resolve
`api_key=` from the top-of-notebook assignment, then the environment.
Never print the key. This tree does not load `.env` files.

Paste a key in the first code cell, or keep using the environment:

```bash
OPENAI_API_KEY=... \
  uv run jupyter notebook examples/python-minimal/notebooks/06_openai.ipynb
```
