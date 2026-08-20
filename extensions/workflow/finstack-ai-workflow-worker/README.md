# finstack-ai-workflow-worker

Leased worker for `finstack-ai-workflow-local`. Polls an adapter-owned wake
index and the local cron table, claims due work with CAS leases, and resumes
runs parked on Timer / Interaction / DeferredEffect waits.

- Sessions become visible to the worker only through `park()`.
- All tables are hints; the kernel journal is authoritative.
- Missed cron fires coalesce into one catch-up fire (inherited from the
  local cron adapter).
- Hosts register a `PortsFactory` per workflow kind and a `RunStarter` per
  cron schedule; the worker never invents ports.
