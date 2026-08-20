# Codex Child Agent (External Peer Invoker) — Design Spec

Date: 2026-08-19
Status: Accepted for v1 implementation
Plan: `docs/superpowers/plans/2026-08-19-codex-child-agent.md`

## 1. Summary

Treat OpenAI Codex as a **child agent product**, not as an OpenAI model. The
parent stays a normal finstack agent (OpenAI / Anthropic / Ollama /
OpenRouter). The parent delegates a coding task through a dedicated toolset;
Codex runs it as a spawned child process using its own `codex login`
credentials. Finstack never reads `~/.codex`.

```text
Parent Agent  --codex_start tool-->  CodexChildInvoker.start_or_attach
                                              |
                                              v
                                    spawn `codex exec --json`
                                              |
                                              v
                                    Codex uses ~/.codex itself
                                              |
                                              v
                                    tool result (status / last message / thread id)
```

This is the FR-05 "external peer" path. The attach seam is
`ChildPlacement::RemoteChildSession` + `AgentInvoker` — not a new provider
crate, and not the PR-058 remote protocol (Codex does not speak PR-058
framing, so `RemoteChildInvoker` cannot be pointed at it).

The same pattern is specified (as a later option, §8) for a **Claude Code
child over ACP**, so one invoker shape covers both external-peer products.

## 2. Existing seams (verified against the codebase)

| Piece | Status | Where |
| --- | --- | --- |
| `AgentInvoker` (`start_or_attach`, `cancel`) | Built | `crates/finstack-ai-runtime/src/services/agent_invoker.rs:108` |
| `ChildRunRequest` / `ChildRunHandle` / `AgentRef` | Built | same file, lines 21–105 |
| `ChildPlacement::RemoteChildSession`, `RemoteRouteRef`, `ChildRunLocator` | Built | `crates/finstack-ai-kernel/src/records/run/child.rs:14–61` |
| `ExternalHandleRef` | Built (immutable; private fields; `try_new`) | `crates/finstack-ai-kernel/src/primitives/handles.rs:17` |
| `ChildRunPrepared` journal record | Built | `crates/finstack-ai-kernel/src/records/run/child.rs:63` |
| `ChildRunPolicy { Deny, Allow { max_depth } }` | Built, enforced in facade | `crates/finstack-ai/src/spec/mod.rs:163`, `agent/child.rs:772` |
| `BudgetPropagation::SharedScope` | Built | `crates/finstack-ai-kernel/src/records/run/propagation.rs:23` |
| `finstack-ai-tools-subagent` (start/status/cancel) | Built; **rejects remote placement** | `extensions/toolsets/finstack-ai-tools-subagent/src/lib.rs:243` |
| `finstack-ai-remote-child` | Built; speaks PR-058 to a finstack peer only | `extensions/interop/finstack-ai-remote-child/` |
| `child_relation_digest(ctx, request)` | Built, exported from runtime | `crates/finstack-ai-runtime/src/services/composition.rs:368` |

### 2.1 Corrections to the original sketch

These change the design, so they are recorded explicitly:

1. **`AgentInvoker` has no `reconcile()` method.** The trait is
   `start_or_attach` + `cancel` only. The `Unknown`-after-crash pattern lives
   on the tool/model ports (`ToolReconcileResult::Unknown`). For this crate,
   run status is an **inherent method on the concrete invoker**
   (`CodexChildInvoker::run_status`) that the toolset reads. After a host
   crash the in-memory run table is gone and status is reported `unknown`
   (v1). Thread-id-based `codex exec resume` is a v2 concern.
2. **`ExternalHandleRef` is immutable and precommitted.** It sits inside
   `ChildRunLocator.remote`, which is part of `request_digest`, so nothing
   discovered after spawn (pid, Codex thread id) can be "persisted on the
   locator". Pid and thread id live in the invoker's in-memory accepted-run
   table, exactly as `RemoteChildInvoker` keeps its `accepted` map. The
   durable record is `ChildRunPrepared`; the route ref is a stable label
   (`finstack.peer.codex` / `"codex-exec"`), not per-run state.
3. **`finstack.peer.*` does not exist yet.** This spec mints it. The closest
   precedent is the service `ComponentId` `finstack.remote.worker`. The peer
   id must be `AgentId::parse` / `ComponentId::parse` compatible (dotted
   lowercase), which `finstack.peer.codex` is.
4. **The toolset builds locator and request digest itself** (no facade
   dependency), mirroring `SubagentToolset`'s `child_locator` /
   `request_digest` helpers.
5. **Every new `extensions/**` crate needs a cargo-public-api baseline** at
   `fixtures/compatibility/public-rust-api/cargo-public-api/<crate>.txt` or
   `mise run check-public-api` fails.

## 3. Decisions

| # | Decision | Rationale |
| --- | --- | --- |
| D1 | v1 protocol is `codex exec --json` (one-shot JSONL). | Maps cleanly onto start / status / cancel. App-server (JSON-RPC threads/approvals) is v2. |
| D2 | **Approvals are frozen at construction; Codex may mutate the workspace autonomously within its sandbox.** Host picks `CodexSandboxMode` explicitly (no default). | `codex exec` is non-interactive; bridging per-command approvals into finstack typed interactions forces app-server as v1 and couples the leaf to an unbuilt interaction design. Mitigations: explicit `workspace_root`, Codex's own sandbox, and the host consciously not granting the parent `shell` on the same tree. |
| D3 | One crate, `extensions/interop/finstack-ai-codex-child`, holds **both** the invoker and the toolset. | Matches the one-crate-per-battery norm (cf. MCP toolset = client + toolset). Avoids a toolsets→interop leaf-to-leaf dependency, one public-API baseline. |
| D4 | The toolset holds the **concrete** `Arc<CodexChildInvoker>` (upcast where the trait is needed). | `run_status` is not on the `AgentInvoker` trait; the kernel learns no Codex types. |
| D5 | Auth stays inside Codex. Construction takes an explicit binary path and workspace root. No env-var discovery, no `~/.codex/auth.json` reader, no keyring. | Fail closed; finstack never holds Codex credentials. |
| D6 | Agent identity is `finstack.peer.codex` with `spec_digest = Digest::domain_separated("codex-peer-spec", 1, b"codex-exec/v1")`. | Codex is not a bundle-resolved finstack `Agent`; the digest is a frozen allow-list identity, not a kernel spec. |
| D7 | No generic "external peer" abstraction in v1. The ACP/Claude invoker (§8) copies the pattern into its own crate later. | YAGNI; abstraction is earned after the second concrete implementation exists. |
| D8 | Budget is observational in v1. `requested_budget` defaults; Codex JSONL usage (`turn.completed.usage`) is surfaced in status output but not charged. | A Codex peer cannot mint or draw finstack budget; mapping usage into a charge is a later, deliberate step. |

## 4. Requirements

### Functional

- FR-1: A parent model can start exactly one prepared Codex child per
  `ChildRunRequest` via `codex_start`, poll it via `codex_status`
  (non-blocking), and cancel it via `codex_cancel`.
- FR-2: `start_or_attach` is idempotent: an equal `request_digest` for an
  already-accepted `RunId` attaches (returns the prior handle, no second
  spawn); a differing digest returns `AgentInvokeError::Conflict`.
- FR-3: Placement is always `RemoteChildSession`; the locator carries
  `RemoteRouteRef { service: finstack.peer.codex@1.0.0, route: "codex-exec" }`.
- FR-4: Status reports `running | completed | failed | cancelled | unknown`,
  plus Codex thread id, last agent message (bounded), token usage, and exit
  code when available.
- FR-5: `cancel` terminates the spawned process; cancelling an unknown
  locator fails closed (`Unavailable`), mirroring `RemoteChildInvoker`.
- FR-6: Construction fails closed when the binary or workspace root is
  missing.

### Non-functional

- NFR-1: T1 native crate; not compiled into wasm-host; README declares tier
  (matches `finstack-ai-remote-child` precedent).
- NFR-2: Standard extension lint header (`forbid(unsafe_code)`,
  `deny(clippy::unwrap_used/expect_used/panic/unreachable)`,
  `warn(missing_docs)`), `[lints] workspace = true`.
- NFR-3: No network I/O from this crate. Only child-process spawn of the
  configured binary.
- NFR-4: Prompt text is passed after a literal `--` argument so a prompt
  beginning with `-` cannot inject flags.
- NFR-5: Accepted-run table capped at 1 024 entries (`MAX_ACCEPTED`,
  matching `RemoteChildInvoker`).
- NFR-6: Tool results bounded: `max_result_bytes: 8_192`; last message
  truncated to 4 096 bytes on a char boundary.

## 5. Architecture

Crate: `extensions/interop/finstack-ai-codex-child`

```text
src/
  lib.rs        lint header, docs, re-exports, error codes
  identity.rs   finstack.peer.codex AgentRef + RemoteRouteRef builders
  config.rs     CodexExecConfig { binary, workspace_root, sandbox,
                skip_git_repo_check, extra_args }, CodexSandboxMode
  events.rs     tolerant JSONL parser -> CodexEvent
  state.rs      CodexRunStatus reducer (events + exit status -> status)
  invoker.rs    CodexChildInvoker: AgentInvoker impl + run table +
                run_status accessor + process supervision task
  toolset.rs    CodexToolset: codex_start / codex_status / codex_cancel
  bin/codex_fake.rs   deterministic stand-in binary for tests
tests/exec.rs   integration tests spawning codex_fake
```

### 5.1 Process contract (v1)

```text
<binary> exec --json --sandbox <read-only|workspace-write|danger-full-access>
         --cd <workspace_root> [--skip-git-repo-check] [extra_args…] -- <prompt>
```

stdout is JSONL. Events consumed (all others ignored as `Other`):

| Event | Effect on run state |
| --- | --- |
| `thread.started` | record `thread_id` |
| `item.completed` with `item.type == "agent_message"` | record `last_message` |
| `turn.completed` | record `usage { input_tokens, cached_input_tokens, output_tokens }` |
| `turn.failed` / `error` | record failure message |
| process exit | `Completed` (code 0, no failure), else `Failed`; `Cancelled` wins if cancel was requested |

The parser is tolerant: unknown event types and malformed lines never fail a
run. (Verify the concrete shapes against the installed Codex CLI during
implementation; the parser must not assume more than the table above.)

### 5.2 Journal and replay semantics

Codex does not write finstack records. The parent journals
`ChildRunPrepared` (facade/coordinator concern, unchanged). Codex internals
are not replayable; after a host restart the invoker's run table is empty
and `codex_status` reports `unknown` for previously started runs. This is
the same posture MCP deferrals take when reconciliation metadata is absent.

### 5.3 Policy and wiring

Host wiring (v1, direct-registration style — the same way
`SubagentToolset` is wired in `crates/finstack-ai-test/tests/lanes/subagent.rs`):

1. Build `CodexChildInvoker::try_new(CodexExecConfig { … })`.
2. Register `CodexToolset::try_new(Arc<CodexChildInvoker>)` as a toolset.
3. Where bundle resolution is in play, the invoker slots into
   `RuntimeServices.agent_invoker` / `HostFeature::AgentInvoker`.
4. Parent `ChildRunPolicy` stays `Deny` by default; the host sets
   `Allow { max_depth: 1 }` when it opts in. (Enforcement is a facade
   concern; the toolset additionally marks all three tools
   `ApprovalRequirement::Required`, as `SubagentToolset` does.)

The toolset holds no spawn authority of its own: every start goes through
the invoker, whose binary/workspace/sandbox were frozen at construction.

## 6. Explicit non-goals (v1)

- A second OpenAI Responses provider, or any `Model` impl for Codex.
- Reading `~/.codex/auth.json`, the keyring, or sending Bearer tokens.
- Teaching the kernel new Codex types.
- Reopening `subagent_start`'s placement enum or touching
  `finstack-ai-remote-child`.
- Pointing `RemoteChildInvoker` at Codex (it speaks PR-058, Codex does not).
- A generic ACP mesh or shared "peer invoker" trait before this one leaf
  works.
- Approval bridging / mid-run "may I run this command?" (v2, app-server).
- Charging Codex token usage against finstack budget (observational only).

## 7. v2 — `codex app-server` (deferred)

Long-lived JSON-RPC surface: threads, turns, approval callbacks, history.
Needed only when the parent must answer Codex approval prompts through
finstack typed interactions. Same crate, second invoker construction mode;
the toolset verbs stay the same. Not designed further here.

## 8. Option — ACP child (Claude Code and other ACP agents)

The same external-peer pattern, specified so the second implementation is a
copy, not a research project. **Own crate, own plan, after Codex v1 ships.**

- Crate: `extensions/interop/finstack-ai-acp-child` (T1 native).
- Identity: `finstack.peer.claude` (or `finstack.peer.acp` +
  per-agent suffix), `spec_digest = Digest::domain_separated("acp-peer-spec", 1, b"acp/v1")`.
- Transport: spawn the ACP agent binary (for Claude Code, the
  `claude-code-acp` adapter) and speak **Agent Client Protocol** JSON-RPC
  over stdio — a long-lived session, i.e. the structural analogue of Codex
  app-server rather than `codex exec`.

Protocol mapping:

| ACP method / notification | Invoker semantics |
| --- | --- |
| `initialize` | handshake at first `start_or_attach`; fail closed on version mismatch |
| `session/new { cwd }` | one ACP session per accepted `ChildRunRequest`; session id → in-memory run table (same immutability rule: never on the locator) |
| `session/prompt` | the child's input (`ChildRunRequest.input` text blocks) |
| `session/update` notifications | reduce into the same `RunState` shape as Codex events (`agent_message_chunk` → last message, `tool_call*` → activity) |
| `session/request_permission` | **v1 ACP: answered from a frozen construction-time policy** (allow-list of tool kinds, else deny) — the exact analogue of Codex frozen sandbox flags. Typed-interaction bridging to the parent is the later step, shared with Codex app-server. |
| `session/cancel` | `cancel` |
| stop response / turn end | `Completed` / `Failed` |

Exec-style alternative for Claude specifically: `claude -p <prompt>
--output-format stream-json` is the direct analogue of `codex exec --json`
(one-shot JSONL, frozen `--permission-mode`). If v1-parity is wanted faster
than ACP, that is the smaller leaf; ACP is the one that generalizes beyond
Claude (Gemini CLI and other ACP agents). Either way: same
`AgentInvoker` + dedicated toolset shape, no kernel changes, no shared
abstraction until both crates exist.

## 9. Testing strategy

- Pure unit tests for identity, config validation, event parsing, and the
  state reducer (in `src/tests.rs` per small-crate convention).
- Process integration tests in `tests/exec.rs` against a deterministic
  `[[bin]] codex_fake` helper resolved via `env!("CARGO_BIN_EXE_codex_fake")`
  (the `finstack-ai-store-sqlite` pattern). Modes (`success`, `fail`,
  `hang`) are selected by a `--fake-mode` flag passed through
  `extra_args`, so parallel tests share no global state.
- No test talks to a real Codex binary or the network.

## 10. Spike (already possible, not this feature)

Giving the parent the existing shell toolset and letting it call
`codex exec` raw proves prompt/result shape only — no placement, no policy,
no cancel fan-out. Useful as manual exploration; not a deliverable.
