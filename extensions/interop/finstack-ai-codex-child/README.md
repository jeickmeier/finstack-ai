# finstack-ai-codex-child

Codex exec child-agent invoker and toolset. A parent finstack agent
delegates a coding task; a locally installed OpenAI Codex CLI runs it as a
spawned `codex exec --json` process using its own `codex login` credentials.
Finstack never reads `~/.codex`.

- `CodexChildInvoker` implements `AgentInvoker` for
  `ChildPlacement::RemoteChildSession` with the frozen peer identity
  `finstack.peer.codex`. Equal `request_digest` attaches; a differing digest
  is a conflict. Run state (thread id, last message, usage, exit) is held
  in memory; status is `unknown` after a host restart, and for settled runs
  evicted from a full run table (an equal replay still attaches via a
  bounded tombstone instead of spawning a second child). Full locators and
  parent ownership remain bound for status and cancellation.
- `CodexToolset` exposes `codex_start` / `codex_status` / `codex_cancel`.
  Construction requires the host-bound `ChildRunStarter`, which enforces the
  frozen policy and commits `ChildRunPrepared` before the invoker can spawn.
  The prompt is the only model-supplied input; binary path, workspace root,
  and sandbox mode are frozen at construction.

This crate is a T1 native adapter. It is not isolated and is not compiled
into `wasm-host`. Codex's sandbox is Codex's own; the host chooses the
sandbox mode explicitly and should not also grant the parent an
unconstrained shell on the same tree.
