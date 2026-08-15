# PR-056 execution plan

Date: 2026-08-14
Owner: me@jeickmeier.com
Intended branch (when admitted): `codex/pr-056-coding-research-batteries`
Intended baseline: local `main` at `40206b1380cac0759b1f68f5fb80e343cc123bfe`
Plan baseline: documentation pack v0.20 / PLAN-0.18 / Implementation Plan SHA-256
`555a150fa9eaa2de39342eabdfd3d050b19628d735adbf498a9d75fcbc1102a4`

This file is the execution contract for PR-056. The closed PR-054
envelope is not reused. The PR-055 envelope is not reused. PR-056 is
the only active logical PR once admitted. This planning file does
not admit the PR, start Phase 8, or record Phase 8 entrance.

## Execution envelope

Authorized by the owner sentence `Proceed to PR-056 through PR-066`
on 2026-08-15, recorded as:

```
Run PR-056 through PR-066 sequentially; mode=integrated; target=main;
local branch/commit/merge authorized; external actions=none;
stop before any gate crossing unless a separate passing gate decision exists.
```

PR-056 is the only active logical PR. Do not start PR-057+ until
this PR is `Done`. Phase 9 (PR-062–PR-066) stays blocked on G7 and
Phase 9 entrance.

```
mode=integrated; target=main
local branch/commit/merge authorized
external actions=none
```

Forbidden: push, hosted PR/merge, npm/pypi publish, crates.io
publish, tag, G7 inference, G8 inference. Do not write `G7-D-*` or
`G8-D-*`. Do not cut or publish `0.1.0`. Do not bump the lockstep
workspace version off `0.0.4`.

## Admission (when authorized)

Do not admit coding until all of the following are true. Planning
this file does not record them.

### Phase 8 entrance is `Passed` (2/2)

Recorded by PR-055. Current state is `Passed` (2/2).

| Entrance bullet | Current state |
| --- | --- |
| Native preview, Python alpha, WASM alpha, durability beta, plugin alpha | **Satisfiable.** G3/G4/G5/G6 are named (`G5-D-durable-beta-a9568bd869b5`). Do not record `PH8-E-entrance-gates-*` until PR-055 is admitted. |
| Public API change backlog triaged | **Blocked.** `docs/implementation/public-api-change-backlog.md` does not exist. |

Do not infer G5 from Phase 6 `Done`. Do not write the backlog or
`G5-D-*` from this PR. If the owner says `implement the plan` while
entrance is still `0/2`, **stop**.

Phase 8 entrance, once Passed, is recorded by the first admitted
Phase 8 PR (PR-055 if it lands first). Do not re-record
`PH8-E-entrance-*` here.

### PR-055 is `Done`

PR-055 is `Done` at local `main` merge
`bfa380340c1bf28941c9cbd21a204626d114052b`. Implementation Plan
lists PR-055 then PR-056. Keep one active logical PR.

PR-056's *code* dependencies are PR-025, the ContextProvider /
Middleware ports (PR-018), and durability where required (PR-048
contract). Those code dependencies are already on `main`. The
sequence dependency on PR-055 is delivery order, not a missing
port.

### Other admission checks

- ADR-037 stays `In progress` / `Partial` until this PR's batteries
  plus existing PR-018/PR-023/PR-048 evidence are bound. Do not
  mark it Implemented unless the security review says the TDD §17.6
  contract is fully exercised by a leaf (G7 still owns preview
  closeout). Never write a superseding ADR unless the existing
  stage/outcome contract cannot preserve required semantics.
- ADR-008 / ADR-020 stay Partial (G5 still owns durability
  closeout). ADR-014 stays Missing.
- No Implementation Plan section 6.3 ADR trigger applies if the work
  adds no seventh port, no eighth middleware stage, no kernel I/O,
  and no native dylib loader. The optional shell sandbox is an
  adapter trait inside the shell crate, not a new port.
- Threat Model section 18 is triggered (high-privilege shell
  battery; secret-bearing / compaction surfaces). Primary **TM-03**
  and **TM-21**. Also **TM-01** (retrieval as data), **TM-20**
  (artifact spill), **SEC-INV-001/007/008/012/013**. Complete the
  review before merge.

## Traceability

Implementation Plan PR-056; PRD UC-02; FR-CTX-003; FR-MW-006;
FR-MW-007; FR-CTX-004; Architecture §11.5; TDD §17.6; ADR-037.
TM-01 / TM-03 / TM-20 / TM-21.
G7 is Phase 8's gate and is out of scope.

## Acceptance mapping

Eight Implementation Plan bullets map 1:1 to A01–A08.

- PR-056-A01: Security tests cover traversal, environment leakage,
  command policy, timeouts, and output floods. Filesystem keeps the
  PR-025 handle-relative no-follow proofs and adds any missing
  harden rows (env leakage through search/read of `.env` remains
  denied; no ambient `std::env` bleed into tool results). Shell
  adds deny-by-default argv policy, empty/allowlisted environment,
  deadline timeout, and output-byte flood → bound or
  `shell_limit_exceeded`.
- PR-056-A02: Context and compaction changes are attributed and
  replay-safe. Repository/memory contributions carry provenance and
  a committed context effect. Compaction writes
  `StageOutcome::CompactContext` / `RequestCompactionModel` through
  the existing PR-018 recorded-outcome path. Replay reuses the
  recorded projection; it does not rerun summarization.
- PR-056-A03: Compaction never mutates canonical history.
  Sliding-window and summarizing strategies produce valid bounded
  requests, invalidate incompatible checkpoints (component /
  strategy / config / model-profile / covered-history /
  sensitivity), preserve protected content, and fail as
  `CONTEXT_BUDGET_EXCEEDED` / existing compaction-invalid codes
  when no valid projection fits. Use
  `finstack-ai-test` `check_compaction_conformance` plus new leaf
  fixtures. Do not add kernel record kinds.
- PR-056-A04: Batteries remain separate leaf packages. Kernel,
  runtime, default SDK, `finstack-ai-wit`, and `check-wasm` graphs
  stay free of shell, compaction, repository, memory, and verify
  crates. No default SDK feature pulls shell.
- PR-056-A05: The coding-agent example uses only public components.
  Extend `examples/rust-minimal/src/bin/coding.rs` (already in the
  TDD example set). Do not invent `examples/coding-agent/` or a
  TUI. The binary may depend on the new leaves; it must not use
  crate-private modules.
- PR-056-A06: Reference memory/retrieval uses public
  `ContextProvider` plus artifact/blob boundaries. The
  `before_finalize` verifier returns only allowed outcomes
  (`Continue`, `Fail`, `RequestInteraction`, accept-the-candidate).
  It does not mutate state after the terminal record and cannot
  `Replace` / `AddContext` / `CompactContext`.
- PR-056-A07: Model-assisted summarization uses an explicit
  model/budget scope, returns `RequestCompactionModel` (never calls
  a provider from the leaf), cannot recursively invoke the same
  compaction chain (depth one), and has cancellation/recovery
  tests against the existing child-effect / resume-cursor contract.
- PR-056-A08: Protected instructions remain byte/ID-identical.
  Generated summaries are provenance-bearing unprivileged
  `DerivedSummary` context. Unauthorized sensitivity / residency /
  egress to a secondary model fails **before** dispatch.

Principal changes that are not extra acceptance IDs, but are
required to prove the eight bullets:

- Harden `finstack-ai-tools-filesystem` in place.
- Implement `finstack-ai-tools-shell` (placeholder README already
  exists).
- Repository-instruction `ContextProvider`.
- One `before_model` compaction leaf that owns window, large-tool
  output, and summarizing strategies.
- Reference memory/retrieval `ContextProvider`.
- `before_finalize` verification example leaf.
- Large-tool-output truncation/spill that preserves pairing.

## Locked design

### Layout

Follow Technical Design §2: trusted native leaves stay under
`extensions/`; do not put them in `crates/` or `plugins/`. The §2
tree already names `finstack-ai-tools-shell`. It does not name
context/middleware leaves; add them as **siblings** under
`extensions/`, not a second root.

```text
extensions/toolsets/finstack-ai-tools-filesystem/     # harden
extensions/toolsets/finstack-ai-tools-shell/          # implement placeholder
extensions/context/finstack-ai-context-repository/    # NEW sibling class
extensions/context/finstack-ai-context-memory/        # NEW
extensions/middleware/finstack-ai-middleware-compaction/  # NEW; ONE owner
extensions/middleware/finstack-ai-middleware-verify/  # NEW example leaf
examples/rust-minimal/src/bin/coding.rs               # public composition
```

Do not create `finstack-ai-tools-ollama`, a second compaction
crate, or `examples/coding-agent/`. Windowing, large-tool-output,
and summarizing are **strategies** of one
`MiddlewareRole::ContextCompactor` component (Architecture §11.5).
Resolution already fails on a second compactor.

Workspace members + `[workspace.dependencies]` at `0.0.4`.
`publish = false` on examples. Calculator stays untouched except
where the coding example already uses it.

### Filesystem harden

PR-025 already has handle-relative `openat` / `NOFOLLOW`, protected
`.git` / `.env` / `.ssh`, symlink/rename races, and ArtifactStore
spill. Do not rewrite the crate.

Add only the missing harden rows:

- Search/read cannot return `.env` / secret-named files through
  glob aliases or case tricks already in policy; add an explicit
  environment-leakage test (tool result contains no
  `std::env::vars` dump and no protected-file bytes).
- Oversized successful results already stage through
  `stage_required_artifact`. Keep that path. When compaction later
  truncates a tool result, the staged `ArtifactRef` remains the
  inspectable body; do not create a second untracked spill file.
- Fail-closed non-Unix construction stays. Do not add a Windows
  backend in this PR unless a capability-safe primitive is already
  available; do not weaken Unix proofs.
- No recursive delete, no ambient root discovery, no Git write
  integration.

### Shell

Replace the placeholder README with a real crate that implements
`Toolset`. Suggested tools (keep the set small):

- `finstack.tools.shell.exec` — argv vector, not a `/bin/sh -c`
  string, unless the policy explicitly allowlists a shell binary
  **and** the caller still passes argv.

Policy (deny by default):

- Allowlist of executable basenames or exact paths. Unknown
  command → `shell_policy_denied`.
- Deny path separators in the program name unless the policy
  listed that exact path. No `$PATH` search unless the policy
  opts in to a bounded search path.
- Environment: start empty (plus a documented tiny locale/PATH
  allowlist if a fixture requires it). Host secrets never copied.
  `env` / `printenv` style leakage tests.
- Working directory: opened handle under an authorized root, same
  no-follow discipline as filesystem when a cwd is supplied.
- Timeout: `CancellationSignal` + deadline. Overrun →
  `shell_timeout` / cancelled, no fabricated success.
- Output: stdout+stderr byte cap; overflow stages through
  ArtifactStore or fails `shell_limit_exceeded`. No untracked
  temp files.
- Optional `CommandSandbox` trait in this crate only:

```text
trait CommandSandbox: Send + Sync {
    fn run(&self, request: SandboxedCommand) -> PortFuture<Result<SandboxedOutput, ToolError>>;
}
```

Default impl is the in-process policy + `std::process` (or rustix
spawn) with the bounds above. A host may inject an external
sandbox. Do not ship landlock/bubblewrap/seatbelt implementations
here. Do not add `libloading`.

Redirects/network: not granted. Shell is T1 trusted-native
(TM-06). Label it as host-authority in the README.

### Repository context

`RepositoryContextProvider` implements `ContextProvider` only.

- Authorized root handle (reuse filesystem path policy; do not
  reimplement a weaker walker).
- Explicit filename allowlist, defaulting to common instruction
  files (`AGENTS.md`, `README.md`, `.finstack/instructions.md`).
  No recursive whole-repo ingest.
- `collect` honors `ContextRequest` budget and
  `ContextOverflowPolicy`. Oversize → omit with diagnostic or
  reject; never silent exceed (FR-CTX-003).
- Items are unprivileged context with file provenance, not System
  instructions, unless the descriptor's
  `trusted_application_instructions` path is explicitly configured
  and tested.
- Treat file text as data (TM-01). No tool execution from
  discovered files.

### Memory / retrieval

`MemoryContextProvider` (and a tiny in-process memory store used
only by that crate) is a **reference pattern**, not a vector-DB
product.

- Write path: application or tool stages bytes through
  `ArtifactStore` / `BlobRef`. The provider does not keep a
  parallel untracked corpus.
- Read path: `collect` returns budgeted `ContextItem`s with
  artifact references, provenance, and sensitivity. No ambient
  cross-tenant deref (TM-20).
- No embedding model, no ANN index, no network fetch. Keyword /
  exact-id retrieval is enough.
- FR-CTX-004: memory is optional policy, not a kernel concept.

### Compaction leaf

One crate, one `MiddlewareRole::ContextCompactor` descriptor,
`OrderTier::ContextCompaction`, `Stage::BeforeModel` only.

Strategies selected by config (not by registering a second
compactor):

| Strategy id | Behavior |
| --- | --- |
| `finstack.compaction.sliding_window` | Drop oldest eligible mutable entries until under threshold; keep protected + current user + unpaired in-flight tools |
| `finstack.compaction.large_tool_output` | Replace eligible large tool-result bodies with inspectable `ArtifactRef` / truncated text; keep call/result pairing and IDs |
| `finstack.compaction.summarize` | Return `RequestCompactionModel` with explicit `CompactionModelRequest` (model, budget_scope, source_sensitivity, residency digest, resume_state) |

Threshold + hysteresis live in the configuration digest. Below
threshold → `Continue` (no rewrite). Prompt-cache impact:
`StablePrefixPreserved` when only the mutable suffix changes;
`CacheInvalidated` when the prefix moves.

Deterministic strategies complete inside `invoke` as
`CompactContext`. Summarize **must not** call `Model::request`.
The runtime already: authorizes the secondary model for full
source sensitivity/residency/egress; allocates a child effect with
`EffectPurpose::CompactionSummary`; commits before dispatch;
resumes the same middleware identity. Leaf tests prove the
negative: a summarize impl that would call a provider is
structurally impossible (no `Model` handle in the crate).

Unauthorized secondary model → fail before dispatch (A08). Recursion
→ existing depth-one check; add a leaf test that a resume cannot
emit another `RequestCompactionModel` for the same chain.

Checkpoints: fill `CompactionCheckpoint` so incompatible
component/strategy/version/config/profile/source/sensitivity
misses rebuild from canonical history. Missing required
projection is integrity failure, not a re-summarize.

Do not change `crates/finstack-ai-runtime/src/middleware.rs`
contracts unless a compile error proves a leaf cannot implement
TDD §17.6. That would be an ADR-037 reconsideration trigger —
stop and use change control.

Diagnostics: evidence already has `estimated_tokens_before/after`
and `cache_impact`. Do not add observer exporters (PR-057). Do
not put source/summary text on observer events.

### Large-tool-output spill

Shared rules for filesystem, shell, and the large-tool-output
strategy:

- Tool-call and tool-result stay paired and source-ordered.
- Spill body is an `ArtifactRef` with digest, length, sensitivity,
  and scope from the committed call context.
- Inline remainder is a bounded preview plus the reference.
- Compaction may replace the model-visible result text; it must
  not delete the canonical `ConversationEntry` or the artifact.

### Verifier example

`finstack-ai-middleware-verify` is a leaf example, not a product
judge.

- `Stage::BeforeFinalize` only. `MiddlewareRole::Standard`.
- Outcomes: accept candidate (`Continue`), `Fail`,
  `RequestInteraction` (approval/form), or a bounded continue
  (existing before_finalize continue/retry shape). No
  `Replace` / `AddInstructions` / `AddContext` / `CompactContext`.
- After a terminal record exists, further `invoke` is not issued;
  tests prove the leaf never writes a store. FR-MW-006.

### Coding example

Update `examples/rust-minimal/src/bin/coding.rs` to compose, using
only public APIs:

- scripted or keyless loopback model (no live key)
- calculator + filesystem + shell (shell policy tight enough for
  the fixture)
- repository context on the example root
- compaction middleware (sliding-window; summarize optional and
  off by default)
- verify middleware that accepts the candidate
- memory store

Do not add a prompt loop, TUI, or browser driver. Keep
`publish = false`.

### Graph and bindings

- `cargo tree` of kernel / runtime / default SDK / wit /
  native-examples-without-the-coding-bin: no shell, no
  compaction, no repository, no memory-context, no verify.
  `finstack-ai-native-examples` may depend on the new leaves
  because the coding binary lives there; that is an example
  package, not the SDK. If the example crate would pull shell
  into a graph check that today forbids privileged batteries,
  feature-gate `coding-batteries` on the example crate rather
  than on `finstack-ai`.
- Do not link shell or compaction into the Python wheel in this
  PR (PR-055 owns curated providers; this PR is Rust leaves +
  one native example).
- Add the new crates to `tools/wasm_package/check.py`
  `FORBIDDEN_WASM` if they pull `tokio` / process / filesystem.
- Do not restore `tools/architecture/`.

## Tasks (when admitted)

Task IDs are minted at admit, not now. Do not start these until
admission checks pass.

1. Tracking: confirm Phase 8 entrance `Passed` (2/2) and PR-055
   `Done`; open `codex/pr-056-coding-research-batteries` from the
   then-current `main` tip. Mark PR-056 `In progress`. Do not
   re-record Phase 8 entrance.
2. Filesystem harden tests + any policy gaps (A01 filesystem
   half).
3. Shell crate: policy, timeout, flood, env leakage, optional
   sandbox trait (A01 shell half, A04 graph).
4. Repository + memory context providers with budget/provenance
   fixtures (A02, A06).
5. Compaction leaf: window + large-tool-output deterministic
   strategies; conformance + checkpoint invalidation (A03, A08
   protected-bytes).
6. Summarize strategy: `RequestCompactionModel`, no recursion,
   cancel/recover, unauthorized egress fail-closed (A07, A08).
7. Verify leaf + coding.rs public composition (A05, A06).
8. TM-03 / TM-21 review, candidate evidence. Stop before
   `G7-D-*`.

## Explicit exclusions

No full coding-agent TUI. No browser automation. No Git write
network. No landlock/bubblewrap product sandbox. No vector
database. No second compaction owner. No seventh port or eighth
stage. No kernel history mutation. No observer/OTel/Prometheus
(PR-057). No Anthropic/Ollama work (PR-055). No remote server
(PR-058). No Python/JS shell battery in this PR. No G5 or G7
decision. No `0.1.0` bump, publish, or tag.

## Validation

- `cargo tree -p finstack-ai-kernel -p finstack-ai-runtime -p finstack-ai --locked`
  — no shell, filesystem is already a non-dependency of those
  crates today; keep it that way; no compaction/context leaves
- `cargo tree -p finstack-ai --locked --no-default-features` — same
- `cargo tree -p finstack-ai-tools-shell --locked` — runtime
  Toolset port only; no kernel, no wasmtime, no pyo3, no
  `libloading`
- `cargo tree -p finstack-ai-middleware-compaction --locked` —
  runtime Middleware port only; no `Model` provider crate
- `cargo test -p finstack-ai-tools-filesystem --offline --locked`
- `cargo test -p finstack-ai-tools-shell --offline --locked`
- `cargo test -p finstack-ai-context-repository --offline --locked`
- `cargo test -p finstack-ai-context-memory --offline --locked`
- `cargo test -p finstack-ai-middleware-compaction --offline --locked`
- `cargo test -p finstack-ai-middleware-verify --offline --locked`
- `cargo test -p finstack-ai-test --test compaction_conformance --offline --locked`
- `cargo clippy --workspace --all-targets --locked -- -D warnings`
- `uv run --no-project python tools/wasm_package/check.py graph`
- `mise run check` after the candidate is otherwise green

Do not require `mise run ci`, Playwright, a hosted matrix, or
Criterion numbers. Do not invent `mise run schema-governance`.

## Suggested authorization sentence

When Phase 8 entrance is `Passed` (2/2), PR-055 is `Done`, and the
owner is ready:

```
Run PR-056; mode=integrated; target=main; local branch/commit/merge
authorized; external actions=none; stop before any gate crossing.
```

A later, separate sentence is required to record `G5-D-*`, triage
the public API change backlog, admit PR-055, or record `G7-D-*`.
Do not infer those from `implement the plan`.
