# Python learning notebooks

Twelve notebooks that teach `finstack_ai.Agent` as the composition root,
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
| [12_evaluation.ipynb](12_evaluation.ipynb) | T2 callbacks + Rust evaluation and SQLite stores | None |

T1 native providers and T2 Python callbacks run in-process. They are not
isolated. See trust levels.

The binding also exposes the full native extension roster for
composition (see the parity catalog in
`fixtures/compatibility/binding-parity/v1/extensions.json`): instruction,
compaction (including model-assisted summarize), verify, redaction, and
tool-policy middleware; repository context; log/metrics/otel/billing/
notify observers; skills activation, calculator, MCP, and skill-import
toolsets; sqlite/postgres journals and local/S3 durable artifact stores.
**`FileSystemToolset` and `ShellToolset` are T2 — trusted, not
sandboxed**: their tools run with the host process's privileges, confined
only by capability-safe roots and deny-by-default allowlists.

## The k-track: the knowledge agent as a product

Five narrative notebooks that tell the knowledge-agent story on this
binding — the analyst path of the three-surface product defined in
[`apps/finstack-knowledge/README.md`](../../apps/finstack-knowledge/README.md)
with platform support in the [capability matrix](../../docs/capabilities.md). All five run
offline and deterministically (scripted models); the shared composition
lives in [`_knowledge.py`](_knowledge.py). It returns a `KnowledgeAgent`: retain
its `.agent` for execution and `.maintain()` for bounded native index maintenance.
The CLI and helper use the same native source implementations; browser search
remains a host-adapter subset. Ingestion notebooks execute the real index effect
and verify its receipt.

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
mise run test-search-python
```

## Offline verification

After building the native extension, run every notebook without live provider
calls:

```bash
mise run test-notebooks
```

This task also runs in Python CI. It sets `FINSTACK_NOTEBOOK_OFFLINE=1` in each
notebook kernel, disabling live credential resolution and Ollama probes even
when credentials or a local model are available. Set the same variable when
launching Jupyter to keep interactive notebook runs offline.

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


## Offline evaluation

[12_evaluation.ipynb](12_evaluation.ipynb) compares two agent configurations on
one frozen dataset using real Rust execution and SQLite journals. It reads paired
quality, usage, cost coverage and failure counts, exports body-free JSONL, and
rescores without subject calls. It requires no credentials. After rebuilding the
extension, run `mise run test-eval-python` to validate Python parity, restart,
cancellation and the full notebook. This notebook also runs in Python CI.
