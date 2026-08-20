# finstack-ai-codex-child

Codex exec child-agent invoker and toolset. A parent finstack agent
delegates a coding task; a locally installed OpenAI Codex CLI runs it as a
spawned `codex exec --json` process using its own `codex login` credentials.
Finstack never reads `~/.codex`.

- `CodexChildInvoker` implements `AgentInvoker` for
  `ChildPlacement::RemoteChildSession` with the frozen peer identity
  `finstack.peer.codex`. Equal `request_digest` attaches; a differing digest
  is a conflict. Run state (thread id, last message, usage, exit) is held
  in memory; after a host restart, status is `unknown`.
- `CodexToolset` exposes `codex_start` / `codex_status` / `codex_cancel`.
  The prompt is the only model-supplied input; binary path, workspace root,
  and sandbox mode are frozen at construction.

This crate is a T1 native adapter. It is not isolated and is not compiled
into `wasm-host`. Codex's sandbox is Codex's own; the host chooses the
sandbox mode explicitly and should not also grant the parent an
unconstrained shell on the same tree.

Design: `docs/superpowers/specs/2026-08-19-codex-child-agent.md`.
