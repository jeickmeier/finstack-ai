# Plan C: Python k-track notebooks

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Checkbox steps.

**Goal:** Five narrative notebooks (`k01`–`k05`) telling the knowledge-agent product story on the Python binding, plus an offline golden-questions pytest. `k05` is the initiative's proof artifact: it opens a CLI-created sqlite session and continues it.

**Architecture:** Notebooks live in `examples/python-notebooks/` beside the existing `01`–`11` tour (which is untouched). A shared `_knowledge.py` module mirrors the Plan A composition in Python (option a per spec §6) so the five notebooks and the pytest build the identical agent. Live-provider cells follow the `_support.py` gating conventions; every notebook must execute fully offline (Ollama-loopback or scripted) so CI can run them.

**Tech Stack:** `finstack_ai` binding (editable checkout), `SqliteDurability`, `MemoryExtension`/`MemoryContextProvider`/`MemoryToolset`/`MemoryObserver`, document toolset, existing notebook kernel packages.

**Spec:** `docs/superpowers/specs/2026-08-28-knowledge-agent-design.md` §9, §11

**Depends on:** Plan A (fixture + composition shape). Plan B only for `k05` (needs a CLI-created journal; until B lands, `k05` creates the "CLI" session via a small Rust-free stand-in cell and is marked accordingly — remove the stand-in when B merges).

## Global Constraints

- Notebooks follow the house conventions: no env reads by the binding, `live_value()` gating, deterministic offline path first, live cells clearly marked optional.
- The Python composition in `_knowledge.py` must list its divergences from the Rust composition in its docstring; an empty list is the goal. Any forced divergence is a binding gap — file it, don't hide it.
- Validation: execute each notebook via the flow in `examples/python-notebooks/README.md`; run the golden pytest from the repo venv (`uv run pytest examples/python-notebooks/test_knowledge_golden.py -q`).
- One commit per task; short imperative subject; do not push.

---

### Task C1: `_knowledge.py` shared composition + golden pytest

**Files:**
- Create: `examples/python-notebooks/_knowledge.py`
- Create: `examples/python-notebooks/test_knowledge_golden.py`

**Interfaces:** `build_knowledge_agent(data_dir: Path, *, provider=..., api_key: str | None = None) -> Agent` mirroring Plan A §6 (sqlite durability, two repository-instruction roots, memory provider/toolset/observer, document toolset, instructions/document-ingest/compaction middleware); `golden_entries()` loading `apps/finstack-knowledge/fixtures/golden.json` by relative path.

- [ ] **Step 1:** Failing pytest: every golden entry runs offline against the Python composition; asserts `must_contain` and that observed event kinds ⊇ `event_kinds_expected` (same assertions as the Rust test — copy thresholds, not code).
- [ ] **Step 2:** Implement `_knowledge.py`; keep the divergence docstring honest.
- [ ] **Step 3:** Pytest green from the venv; `mise run check-python` clean on the new files.
- [ ] **Step 4:** Commit `Add python knowledge composition and golden conformance test`

### Task C2: `k01` ingest-and-ask + `k02` events-and-trace

**Files:**
- Create: `examples/python-notebooks/k01_knowledge_ingest.ipynb`
- Create: `examples/python-notebooks/k02_events_and_trace.ipynb`

- [ ] **Step 1:** Outline cells as markdown first (narrative: what/why per cell), reviewed against spec §9.
- [ ] **Step 2:** `k01`: build via `_knowledge.py`; ingest a small bundled markdown; ask; show the answer citing the doc. `k03`-style teasers forbidden — each notebook self-contained. `k02`: same session; iterate `EventBatchIterator`, tabulate event kinds, show `RunResult.trace` and `active_capabilities`.
- [ ] **Step 3:** Both execute clean offline via the README flow.
- [ ] **Step 4:** Commit `Add knowledge ingest and event-stream notebooks`

### Task C3: `k03` memory-across-sessions + `k04` provenance

**Files:**
- Create: `examples/python-notebooks/k03_memory_across_sessions.ipynb`
- Create: `examples/python-notebooks/k04_provenance.ipynb`

- [ ] **Step 1:** Outlines reviewed.
- [ ] **Step 2:** `k03`: session 1 `remember`s facts (tool-driven); fresh session recalls them via the context provider; show the recall budget behavior. `k04`: for one answer, list contributed context items with provenance (repository docs vs memory vs ingested document) and connect them to the citation in the answer.
- [ ] **Step 3:** Execute clean offline.
- [ ] **Step 4:** Commit `Add memory recall and provenance notebooks`

### Task C4: `k05` cross-surface session open

**Files:**
- Create: `examples/python-notebooks/k05_cross_surface.ipynb`
- Modify: `examples/python-notebooks/README.md` (k-track section + parity pointer to `apps/finstack-knowledge/README.md`)

- [ ] **Step 1:** Outline reviewed; states the claim being proven in the first cell.
- [ ] **Step 2:** Create a journal with `finstack-know ask` (or the documented stand-in until Plan B lands); open it with `SqliteDurability` + `Agent.open_session(session_id, tenant_scope)`; list lanes, render history; continue the conversation from Python via `Lane.run` on the opened session's `main` lane (`open_session` itself only inspects; `Lane.resume` is for parked runs); print the command to re-open in the CLI and what the user should see.
- [ ] **Step 3:** Executes clean; README updated.
- [ ] **Step 4:** Commit `Add cross-surface session notebook and k-track docs`
