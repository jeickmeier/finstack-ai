# Plan D: `browser-knowledge` TypeScript example

> **For agentic workers:** REQUIRED SUB-SKILL: superpowers:subagent-driven-development or superpowers:executing-plans, task-by-task. Checkbox steps.

**Goal:** A browser example proving the embedded path: the knowledge agent running in a worker with an IndexedDB journal and a **TypeScript-implemented retrieval toolset** over an in-page corpus, streaming the same event kinds, with golden-question conformance under Playwright.

**Architecture:** `examples/browser-knowledge/` sibling of `browser-minimal`, same build/stage flow (`mise run build-wasm`, `scripts/wasm_package/stage.py`). The retrieval toolset registers through the existing host adapter surface (JS-side `Toolset` via the wasm host bindings — the same seam `host_toolset.rs` serves); the corpus is a few bundled markdown docs (subset of the Plan A self-docs, imported as raw strings). Scripted model scenario by default; no live provider, no server.

**Tech Stack:** `@finstack/ai` packed tarball, TypeScript, Playwright (existing wasm test harness), IndexedDB adapters.

**Spec:** `docs/superpowers/specs/2026-08-28-knowledge-agent-design.md` §10, §11

**Depends on:** Plan A for the golden fixture and self-docs content. Independent of Plans B/C.

## Global Constraints

- Typecheck against the packed tarball like `ts-alpha-install` — no `src/` path mapping. Trust class note in the README (T2 host adapters, not isolated).
- No provider credentials anywhere in the example.
- Validation: the example's Playwright spec runs inside the existing `mise run test-wasm` scope (Chromium-only acceptable, matching `test-fast`); `mise run check-wasm` clean.
- Note: a wasm linked-constructor test is a known flake under parallel workspace load — rerun in isolation before diagnosing new failures.
- One commit per task; short imperative subject; do not push.

---

### Task D1: Example scaffold + worker + IndexedDB journal

**Files:**
- Create: `examples/browser-knowledge/{README.md,index.html,main.ts,worker.ts,tsconfig.json,package.json}`
- Modify: `scripts/wasm_package/stage.py` (register the example in the stage/typecheck flow, same as browser-minimal)

- [ ] **Step 1:** Failing check — stage script typechecks the new example (initially absent → red).
- [ ] **Step 2:** Copy the browser-minimal shape: `connectWorker`, create agent (scripted scenario), run one question, stream `run.events()` kinds into the page, `inspectSession` panel, IndexedDB persistence + clear button.
- [ ] **Step 3:** Stage + typecheck green.
- [ ] **Step 4:** Commit `Scaffold browser-knowledge example with worker and indexeddb`

### Task D2: TypeScript retrieval toolset over the host adapter seam

**Files:**
- Create: `examples/browser-knowledge/retrieval.ts` (corpus + toolset)
- Modify: `worker.ts` (register the toolset with the agent build)

**Interfaces:** `createRetrievalToolset(corpus: {id, title, body}[])` returning the JS toolset shape the host adapter surface accepts: one `search_corpus` tool (query → top-k excerpts with doc ids) with a JSON schema, deterministic ranking (plain term scoring — no dependency), and result content referencing doc ids so answers can cite.

- [ ] **Step 1:** Failing unit tests (vitest/node test per the js package's existing test style) for ranking determinism and schema validity; failing Playwright step: a question whose scripted response calls `search_corpus` and weaves the excerpt into the answer.
- [ ] **Step 2:** Implement; corpus = 3–4 self-doc excerpts imported as raw strings.
- [ ] **Step 3:** Green; typecheck; record any host-adapter friction as binding issues (that surface is the point of this plan).
- [ ] **Step 4:** Commit `Add typescript retrieval toolset via host adapter seam`

### Task D3: Golden-questions conformance under Playwright

**Files:**
- Create: `examples/browser-knowledge/golden.spec.ts`
- Modify: `main.ts` (a "run golden set" button driving the fixture; results table)

- [ ] **Step 1:** Failing spec: load `apps/finstack-knowledge/fixtures/golden.json` (staged into the example at build time — no fetch of repo paths at runtime), run each entry against the scripted scenario, assert `must_contain` and event-kind coverage — the same two assertions as Rust and Python.
- [ ] **Step 2:** Implement; stage the fixture copy in `stage.py`.
- [ ] **Step 3:** Spec green in the wasm Playwright flow; README documents the parity matrix row deltas (IndexedDB is origin-local ⇒ cross-surface = inspect/export only).
- [ ] **Step 4:** Commit `Add golden conformance spec to browser-knowledge`
