# Python learning notebooks

Eleven notebooks that teach `finstack_ai.Agent` as the composition root,
plus the five-notebook **k-track** product narrative (see below).
There is no Python `Harness` type. A harness is the recipe: pick a
provider factory, attach trusted Python ports, run, and inspect events.

The workspace pins `finstack-ai==2.0.0`; use the editable repository package
until a 2.0 package is published.

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
uv run jupyter notebook examples/python-notebooks
```

## Trust and network

| Notebook | Trust | Network |
| --- | --- | --- |
| [01_orientation.ipynb](01_orientation.ipynb) | Import only | None |
| [02_first_agent.ipynb](02_first_agent.ipynb) | T2 callback | None |
| [03_tools_and_structured_output.ipynb](03_tools_and_structured_output.ipynb) | T2 callback + Pydantic extra | None |
| [04_runs_events_sessions.ipynb](04_runs_events_sessions.ipynb) | T2 callback | None |
| [05_ollama_and_harness.ipynb](05_ollama_and_harness.ipynb) | T1 provider + T2 ports | Construct-only unless local Ollama has `gemma4:26b` |
| [06_openai.ipynb](06_openai.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `OPENAI_API_KEY`. Set `OPENAI_MODEL`, `OPENAI_REASONING_EFFORT`, and `OPENAI_REASONING_SUMMARY` in the first code cell |
| [07_anthropic.ipynb](07_anthropic.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `ANTHROPIC_API_KEY` |
| [08_document_ingestion.ipynb](08_document_ingestion.ipynb) | T2 callback | None |
| [09_openrouter.ipynb](09_openrouter.ipynb) | T1 provider + T2 ports | Construct-only unless a key is set in the notebook or `OPENROUTER_API_KEY`. Set `OPENROUTER_MODEL`, `OPENROUTER_REFERER`, `OPENROUTER_TITLE`, `OPENROUTER_REASONING_EFFORT`, and `OPENROUTER_REASONING_SUMMARY` in the first code cell |
| [10_elicitation.ipynb](10_elicitation.ipynb) | T2 callback | None |
| [11_memory.ipynb](11_memory.ipynb) | T2 callback + native memory extension | None |

T1 native providers and T2 Python callbacks run in-process. They are not
isolated. See trust levels.

## The k-track: the knowledge agent as a product

Five narrative notebooks that tell the knowledge-agent story on this
binding — the analyst path of the three-surface product defined in
[`apps/finstack-knowledge/README.md`](../../apps/finstack-knowledge/README.md)
(which also carries the cross-surface parity matrix). All five run
offline and deterministically (scripted models); the shared composition
lives in [`_knowledge.py`](_knowledge.py), whose docstring lists its
honest divergences from the Rust definition.

| Notebook | Story beat |
| --- | --- |
| [k01_knowledge_ingest.ipynb](k01_knowledge_ingest.ipynb) | Build the composition, ingest a document, ask, cite |
| [k02_events_and_trace.ipynb](k02_events_and_trace.ipynb) | The `RunEventKind` stream every surface shares; `RunResult.trace` |
| [k03_memory_across_sessions.ipynb](k03_memory_across_sessions.ipynb) | Remember in one session, recall in a fresh one, budget behavior |
| [k04_provenance.ipynb](k04_provenance.ipynb) | Attribute one answer to memory, instructions, and the ingested doc |
| [k05_cross_surface.ipynb](k05_cross_surface.ipynb) | Open a CLI-created sqlite session, continue it, hand it back |

The golden-questions conformance test
([test_knowledge_golden.py](test_knowledge_golden.py)) runs the shared
fixture `apps/finstack-knowledge/fixtures/golden.json` against this
composition:

```bash
uv run pytest examples/python-notebooks/test_knowledge_golden.py -q
```

## Live cells

Notebooks 01–04, 08, 10, and 11 stay offline. 05 constructs `Agent.ollama` with
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
  uv run jupyter notebook examples/python-notebooks/06_openai.ipynb
```
