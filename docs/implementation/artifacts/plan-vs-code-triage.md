# Plan-versus-code triage

Companion to [plan-vs-code-audit.md](plan-vs-code-audit.md). Each surviving finding is routed to **Fix** (change the code), **Document** (amend the authoritative doc and keep the shipped behaviour), or **Build for 1.0** (§E — architectural work accepted into 1.0 scope, needing its own plan envelope).

## How "Document" works here

Per `docs/planning/README.md`, the Implementation Plan cannot silently override a requirement, standard, or control obligation. So "update the docs to allow it" is a four-part transaction, not a ledger edit:

1. Amend the **authoritative** document (PRD NFR row, Engineering Standards rule, Threat Model control, or the Plan's PR/exit bullet) and bump its controlled-document version.
2. Record an `EX-*` row in `exceptions-register.md` when the deviation is time-bounded, or make it a permanent scope change in the PRD/Plan when it is not.
3. Reconcile `evidence-register.md` and `delivery-ledger.md` rows that cite the removed mechanism.
4. Bump the pack version in `docs/planning/README.md`.

Anything routed to **Document** below that lacks step 1 or 2 today is currently an *undocumented* deviation, which is the actual defect.

---

## A. Fix — broken at HEAD (regressions, no governance question)

| # | Item | Evidence | Fix |
| --- | --- | --- | --- |
| A1 | `cargo clippy -p finstack-ai-runtime --no-default-features -- -D warnings` fails | `crates/finstack-ai-runtime/src/ports/tool/mod.rs:26` unused import | Gate the import on the feature |
| A2 | `mise run runtime` fails with 10 compile errors | `exec/coordinator/tests/fixtures.rs:84,107`; `budget.rs:208` — `KernelInput` undeclared | Fix test imports after the `a47bd31` module move |
| A3 | Browser journal known-answer test asserts 39 against a 40-fixture corpus | `bindings/finstack-ai-wasm/js/src/journal.test.ts:36-37` vs `fixtures/compatibility/journal/v1/*/valid--*.json` (40) | Change to 40; Python and Rust already assert 40 |
| A4 | `cargo package` fails for 17 crates | `readme = "README.md"` in 17 manifests, files deleted in `bb6ac86`/`8756d71`/`8230fa7` | Drop the `readme` key (deletion was deliberate) or restore the files |
| A5 | `tools/docs/quickstarts.py` raises `FileNotFoundError` | `tools/docs/quickstarts.py:30` → `crates/finstack-ai/README.md` | Re-point at `docs/site/` quick starts; amend the PR-060 bullet that says *package* quick starts |
| A6 | `tools/docs/links.py` reports 5 broken links | `docs/site/middleware.md` ×2, `plugin.md`, `server.md`, `plugins/finstack-ai-wit/COMPATIBILITY.md` | Repair links |

A4/A5 share one root cause: the README-removal commits did not update the manifests or the checkers.

## B. Fix — real exposure

| # | Item | Evidence | Why fix, not document |
| --- | --- | --- | --- |
| B1 | 1.0 freeze gate cannot see added fields or changed signatures | `tools/compat/public_items.py:127` reports only removals | The whole point of the frozen surface; two live source breaks are invisible to it |
| B2 | Two unclassified source-breaking public API changes | `SessionRuntime::existing` return type (`services/session.rs:305`); `AgentRunRequest::capability` (`agent/types.rs:75`) with no `#[non_exhaustive]` | Nothing has shipped (`## [Unreleased]`), so classifying now is free; after release it is a major bump |
| B3 | Shell toolset reads both pipes unbounded before checking the cap | `extensions/toolsets/finstack-ai-tools-shell/src/lib.rs:543,546` then `:548` | Open finding FIND-064-007; hostile tool output is an OOM |
| B4 | Reference-server receipt map is uncapped with a linear conflict scan | `crates/finstack-ai-server/src/session.rs:36`, insert `:200`, scan `:192` | FIND-064-003, and `release-engineering.md:109` says it is **not accepted** |
| B5 | JS host stream adapters buffer without bound in wasm linear memory | `bindings/finstack-ai-wasm/src/host.rs:546,589`; `host_model.rs:277` | Same class as B3, pre-drain; downstream bounding does not help |
| B6 | Free-threaded 3.14t test reports PASSED without running | `bindings/finstack-ai-python/tests/test_import.py:104` bare `return` | A silently-passing test is worse than none; minimum fix is `pytest.skip` |
| B7 | NFR-PERF-005 artifact records FAIL while the summary asserts PASS, and the harness cannot regenerate its own report | `artifacts/pr-063/python-fast-path-report.json:40`; `nfr-perf-summary.md:9`; `tools/perf/python_fast_path.py:239,300` | Published evidence contradicts itself — resolve by fixing the metric **or** filing an `EX-*`, but not by leaving both |

## C. Fix — cheap, high value

| # | Item | Fix |
| --- | --- | --- |
| C1 | 43 evidence rows cite `mise` tasks that no longer exist (`architecture`, `docs`, `test-miri`, `schema-governance`, `check-minimal`, …) | Rewrite the rows to cite what actually runs |
| C2 | `exceptions-register.md:39` requires `tools/architecture/allowlist.toml`, which does not exist | Point the waiver process at `deny.toml` or restore a real allowlist |
| C3 | `schema-change-template.md:41` and `CHANGELOG.md:103` still tell contributors to run `mise run schema-governance` | Remove or repoint |
| C4 | `delivery-ledger.md:363,111` calls PR-067 "uncommitted"; it is committed at HEAD. `:525,:526` link deleted workflow/tool paths | Correct the rows |
| C5 | `artifacts/pr-032/implementation.md:13` documents a token-overlap activation policy the code never had and PR-067 explicitly removed | Correct the artifact |
| C6 | `nightly.yml:23` sets `FINSTACK_CANARY_LABEL: "0.1.0-canary"` against a lockstep `1.0.0`, contradicting `release-engineering.md:8`; nothing reads it | Fix the label or drop the variable |
| C7 | `crates/finstack-ai-server` and `crates/finstack-ai-test` are neither staged nor marked `publish = false` | Pick one |
| C8 | `schemas/schema-families.toml:47,57` mark `remote` and `process` active with no fixtures; the file is read by no code | Mark reserved (matches `schemas/README.md:22`) |
| C9 | Staged npm manifest says `0.0.2`; `package.json` says `1.0.0` | Restage or correct |
| C10 | Deterministic, fast checks exist but are in no pipeline: `docs-links`, `docs-quickstarts`, `licenses`, `check-guest-sdk`, `check-plugin-wasm`, `check-plugin-template`, `check-plugin-lock` | Add to `[tasks.ci]` — they are seconds, and C1–C4 keep recurring without them |
| C11 | Fuzz targets exist but nothing invokes them | Add `mise run fuzz-smoke` to `nightly.yml` — the targets and task already exist |

## D. Document — deliberate reductions, keep the code as-is

All of these were owner decisions. The code is fine; the **plan text still demands the deleted mechanism**, which is what makes them findings.

| # | Item | Amend |
| --- | --- | --- |
| D1 | Single `ubuntu-24.04` CI; no macOS/Windows matrix | PRD **NFR-PORT-001**; Plan PR-003/PR-061. Envelope already exists at `artifacts/pr-032/linux-only-envelope.txt` — promote it to an `EX-*` row |
| D2 | No CPython/platform wheel matrix; `python = "3.14"` only | PRD **NFR-PORT-002**; Plan PR-027/PR-032 |
| D3 | Miri/sanitizer suite removed | Plan PR-020 exit criteria. Also correct `artifacts/pr-020/README.md:27`, which still claims the Miri task passes |
| D4 | `tools/architecture/` checker deleted; `ARCH001`/`ARCH002` are prose only | Engineering Standards **ENG-ARCH-001/003**. Name what actually runs: `tools/wasm_package/check.py:110-176`, cargo-deny across six triples, four graph tests |
| D5 | Schema-governance gate deleted | Engineering Standards §155; Plan PR-004. Per-family coupling survives in Rust tests — describe that instead |
| D6 | No benchmarks in any pipeline; no regression comparator; NFR-PERF-001/002/006 have no fail path | Plan §18.2 and `perf-budgets.md`. State plainly that budgets are measured locally and only size budgets gate CI |
| D7 | Size budgets: `python-wheel` and `minimal-cli` are `required: false` and unfailable | `perf-size-budgets.json` + `perf-budgets.md`. Either mark them advisory or build the artifacts — do not leave them looking enforced |
| D8 | No accessibility check | Plan PR-060 — the bullet was never in PR-060's own A01–A06 |
| D9 | WIT guests are Rust-only; no cross-language fixture components | Plan PR-049 — PR-049's own A01–A07 never required it |
| D10 | Plugin-host and provider benches exist but are unratified and unrun | `perf-budgets.md` workload table — ratify or mark warning-only |
| D11 | No compaction / token-cache benchmark | Plan §18.2 |
| D12 | `ts-alpha-install` typechecks but does not run | Plan PR-038 wording ("installs and runs") — runtime execution deliberately lives in the Playwright harness |
| D13 | npm alpha unsigned; `npm-release-staging.yml` never dispatched | Plan PR-038 |
| D14 | No external-adopter soak; **completion criterion 12** unmet | Plan §21. Already accepted at `delivery-ledger.md:109` **without an `EX-*` row** — file one |
| D15 | GA publication, tag push, announcement never executed | Plan PR-066. Already recorded; keep `docs/site/migration.md:14` git-consumption guidance accurate |
| D16 | PR-023 A05 "cross-binding parity" clones one Rust vector three times; PR-034 prebeta adapters are validators plus a status echo | Correct the two evidence artifacts to say what was actually proved — both were descoped in their own PR plans |
| D17 | Interaction resolve on the SDK/Python path bypasses the shared router | Plan PR-044 — kernel principal/authorization/expiry checks still apply; document the two paths |
| D18 | Successful external model completions rejected (`model_response_decoder_unavailable`) | Already documented as an intentional control in `artifacts/pr-042/security-review.txt:22`; reconcile the Plan PR-042 bullet |

### One I would not document away

**Repo-wide secret scanning / gitleaks** (`.gitleaks.toml`, `tools/security/secret_canary.py`, `fixtures/security/canary-redaction/` all deleted in `f915cbf`). The surviving scan covers 14 hard-coded roots and only `.ts/.js/.md/.html`, so no `.rs`, `.py`, or fixture file is scanned. This is **TM-04**, a threat-model control obligation rather than a convenience gate, and gitleaks is a few lines of config. Restore it rather than amending the Threat Model. `preview-feedback-review.md:118` already lists it as an unclosed residual.

## E. Build for 1.0 — decided

Three architectural items are **in scope for 1.0**. These are feature work, not clerical, and each needs a plan envelope (Phase 10 or a new Phase 11 PR) rather than a ledger edit.

### E1 — `ContextProvider` production driver · **Build**

The one genuine architectural hole. The port is registered, resolved and lock-attested, but never invoked: `assemble_context` and `CommittedContextCall::try_new` have only test callers, stated outright at `crates/finstack-ai-runtime/src/exec/middleware_driver/mod.rs:111`.

Cascade — four other findings collapse into this one:

- Repository and memory context leaves are dead code in production (`extensions/context/finstack-ai-context-repository/src/lib.rs:136`, `.../memory/src/lib.rs:244`); the only non-test consumer of `context_providers()` is lock attestation (`crates/finstack-ai/src/bundle/compose.rs:154`).
- The Python `ContextProvider` adapter is complete but unreachable (`bindings/finstack-ai-python/src/callbacks/context_provider.rs:18-108`, wired at `src/agent.rs:170`).
- Compaction middleware can land **neither** outcome: `protected` is hard-coded `false` for every source entry (`exec/stage_settlement/input.rs:137`) because `protected` is authoritative-from-the-context-port, and `RequestCompactionModel` is refused as `MIDDLEWARE_STAGE_UNLANDABLE` (`exec/middleware_driver/fold.rs:12`).
- The `coding` example's compaction leg fails with `compaction_result_invalid` (`examples/rust-minimal/README.md:12`).

Minimum shape: a driver that calls resolved providers at the `before_model` boundary, feeds `assemble_context` (`ports/context/assembly.rs:48`), and constructs protected `ContextItem`s so `stage_settlement` source entries carry real `protected` values. Landing it should make the compaction outcomes reachable without touching `fold.rs`'s refusal logic — verify that, because if it does not, compaction needs its own envelope.

Acceptance should include the Python adapter firing end-to-end, since it is currently untestable.

### E3 — Basic workflow adapter, with cron · **Build, before Temporal**

Decision: build a **simple in-process workflow adapter first**, supporting both **manually started** and **cron-scheduled** workflow starts. Temporal comes after, on the settled model.

**The data model is already largely settled** — this changes the shape of the work. `crates/finstack-ai-runtime/src/driver/workflow/mod.rs` is a ~776-line runtime-driver contract, explicitly "not a seventh port" (`:1-5`), already carrying:

| Piece | Location |
| --- | --- |
| `WorkflowWait`, `classify_wait(state)` | `:41`, `:163` |
| `WorkflowRetryDecision`, `retry_decision(...)` | `:72`, `:252` |
| `WorkflowCheckpoint` (tenant/session/lane/run, `last_applied_seq`, `external_handles`) | `:88` |
| `WorkflowDriverError` + stable codes (`retry_limit_reached`, `retry_not_safe`, `effect_not_outstanding`) | `:106`, `:21-23` |
| `WorkflowSession::attach` / `with_ports` / `persist_handoff` / `abort_owner` | `:446`, `:585`, `:614` |
| Injected `ExternalClock` — "durable sleep advances only through this clock" | `:472` |
| `SeededRandom` for deterministic replay | `:337` |

So the gap is **not** an unsettled model; it is that nothing in production *drives* this contract. `extensions/workflow/finstack-ai-workflow-temporal` is a 154-line shim over it that never touches an engine, and `finstack-ai-workflow-local` was deleted in `b05be2c` (a 14-line `lib.rs` plus a **903-line** `tests/reference.rs`). The test suite survives, relocated to `crates/finstack-ai-test/tests/local_workflow/` (`driver.rs`, `restart.rs`, `tenant.rs`), covering adapter-versus-direct journal equivalence and conflicting-checkpoint handling. **That suite is the executable spec** — promote it into a shipped crate that drives `WorkflowSession`.

**Cron is net-new.** `grep -rniI "cron|schedule"` across the kernel, runtime and workflow extension returns no scheduler — only `SnapshotSchedule` and unrelated `scheduled_at` timestamps. Design constraint: the kernel is deliberately clock-free (PR-011 ships timer *intent* without a kernel clock), so a cron trigger must not become a kernel concern. The clean seam is the adapter, which already owns time: `WorkflowSession::clock()` returns the injected `ExternalClock` through which all durable sleep advances. A cron trigger is then a scheduled adapter-side start of a `WorkflowSession`, not a new kernel record or a seventh port.

Minimum shape:

- Trigger model covering manual start and cron expression, owned by the adapter crate.
- Schedule state durable in the journal (or explicitly documented as adapter-local and lost on restart — pick one and say so; a cron that silently forgets its schedule across a restart is worse than none).
- Missed-fire policy on restart: skip, run-once-catch-up, or backfill. Name it.
- Deterministic replay: cron firing must advance through the injected clock, so `crates/finstack-ai-test/tests/local_workflow/restart.rs` can assert schedule behaviour without wall-clock flake.
- Tenant scoping on schedules, matching `WorkflowSession::tenant_scope()`.

Temporal then becomes a second implementation of a proven driver, and the PR-059 "reference integration" claim becomes true of the local adapter rather than aspirational of the Temporal one. Note `finstack-ai-workflow-temporal/src/lib.rs:148` is currently `let _ = policy_attempts;` — retry accounting is asserted but not applied.

### E4a — Lane surface · **Build**

PR-047's minimum `Lane` surface (`artifacts/pr-047/plan.md:218-228`) is navigate / run / cancel / suspend / resume / inspect. Shipped today in `crates/finstack-ai/src/session/mod.rs`: `navigate` (`:282`), `inspect` (`:296`), `cancel` (`:305`), `append_text` (`:325`). **Missing: `run(input) -> Run`, `suspend()`, `resume()`.** `append_text` appends a user message on an idle lane; it does not start a run.

Sequencing constraint — E4a lands on top of E3. `Session::open` is inspect-not-continue, and its own doc comment says so explicitly:

> "Resume belongs to an explicit workflow driver when one is composed." — `crates/finstack-ai/src/session/mod.rs:58`

The runtime rebuilds the projection "without respawning non-terminal runs" (`crates/finstack-ai-runtime/src/services/session.rs:259`). `resume()` needs that respawn path — which is exactly what `WorkflowSession::with_ports` exists for ("ports used when the driver must spawn `RunTaskOwner`", `driver/workflow/mod.rs:446`) and what `local_workflow/restart.rs` exercises. Build E3 first, then `suspend`/`resume` on top; `run(input)` is independent and can land earlier.

### E4b — Lane surface in Python · **Build**

`PySession` and `PyLane` already exist and are exported (`bindings/finstack-ai-python/src/lib.rs:89-90`), so this is filling in verbs, not a new handle type. Current Python surface (`bindings/finstack-ai-python/src/session.rs`):

- `PySession`: `tenant_scope` (`:21`), `session_id` (`:26`), `create_lane` (`:32`), `list_lanes` (`:52`), `lane` (`:68`), `bind_external_identity` (`:79`), `resolve_external_identity` (`:98`). Missing versus Rust: `lane_by_id`.
- `PyLane`: `lane_id` (`:122`), `session` (`:127`), `navigate` (`:134`), `inspect` (`:147`). **Missing `cancel` and `append_text`, which Rust already has** — plus `run`/`suspend`/`resume` once E4a lands.

So Python lanes are 2 verbs against Rust's 4 and PR-047's planned 6. `cancel` and `append_text` can land immediately and independently of E4a; the rest follows E4a.

### E4c — Python durable store · **Build**

`bindings/finstack-ai-python/src/agent.rs:17` imports only `MemoryJournalStore`, constructed at `:447` and `:495`; `finstack-ai-store-sqlite` appears in no binding manifest. `tests/test_durable_restart.py:1` self-documents that `open_session` does not respawn parked runs — the same inspect-not-continue constraint as E4a, so the respawn half follows E3.

Scope: expose the SQLite store to Python with its durability modes (`extensions/stores/finstack-ai-store-sqlite/src/config.rs:21`), plus the missing Python migration and settlement-idempotency fixtures called for by `artifacts/pr-048/plan.md:415`. Pairs with E4b — a durable lane you cannot reopen from Python is half a feature.

### E4d — Python child-runs and external completions · **Build**

Today the entire surface is `normalize_prebeta_shape` (`bindings/finstack-ai-python/src/protocol.rs:52`), documented as "This data-only API does not route a command" (`python/finstack_ai/__init__.py:101`). Note the **underlying Rust `AgentRun` has no child-run surface either** (`crates/finstack-ai/src/agent/run.rs:145-367`), so this is not a binding-only gap: the Rust surface has to exist first. Interaction request/resolution is already delivered on both sides.

Sequence this last of the E4 group, and confirm whether the Rust child-run surface is in 1.0 scope on its own or only as the substrate for Python.

## Still documented, not built

- **E2 — reference server never drives a run** (`crates/finstack-ai-server/src/session.rs:199` hard-codes `success = true`, never reads `command.op()`; no `RunEvent` → `RemoteEventView` conversion). Relabel as protocol-conformance-only; the accept loop is already marked non-production. Framing, handshake, TLS, auth, credit windows and idempotency are real.

## Suggested order

1. **A1–A6** — the tree is red; nothing else is trustworthy until it is green.
2. **B1–B2** — free before release, expensive after.
3. **B3–B5** — the two open security findings plus their sibling.
4. **C10–C11**, then **C1–C9** — wire the cheap gates first so the register drift stops recurring.
5. **E1** — independent of everything else, and the largest correctness win; four findings close with it. Can run in parallel with the E3/E4 chain.
6. **E3** — the basic workflow adapter plus cron. Unblocks every `resume`-shaped item below it.
7. **E4a**, then **E4b/E4c** — Rust lane verbs, then the Python lane verbs and durable store. Three pieces can land ahead of E3: `Lane::run(input)`, Python `cancel`/`append_text`, and `lane_by_id`.
8. **E4d** — Python child-runs, last, and only after deciding whether the Rust `AgentRun` child-run surface is 1.0 scope in its own right.
9. **D1–D18** as one governance transaction with a single pack-version bump, once the E-group has settled what is actually shipping.

### Plan envelopes

E1, E3 and E4a–E4d are 1.0 scope and add public API, so each needs a plan envelope before code. Phase 10 is open (PR-067 `In progress`) but its stated scope is fail-closed hardening with **no new surface** and an explicit "Explicitly excluded" list — these do not belong in PR-067. They need new PR envelopes, most likely a Phase 11, which also means:

- New entries in the Plan's §5.1 critical-path graph and §22 traceability summary.
- A `1.0-compatibility-matrix.md` decision per item: additive-only, or a `1.1` minor. `Lane::run`/`suspend`/`resume`, `AgentRunRequest`, and the Python handles are all additive; the workflow-driver crate is new-package.
- B1 (`tools/compat/public_items.py` detecting only removals) fixed **first** — otherwise none of this new public surface is visible to the freeze gate.

