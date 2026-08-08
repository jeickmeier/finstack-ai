# Architecture fixtures (PR-002)

Lightweight case files used by architecture unit tests. They do not mutate the
production workspace.

| Path | Purpose |
| --- | --- |
| `cases/` | Waiver schema and source-guard snippet cases |
| `forbidden-kernel/` | Forbidden kernel dependency names for synthetic metadata tests |

Compile-time six-port / native-vs-WASM port-bound proofs are deferred until the
production port traits exist (PR-014–PR-018). Run checks via `mise run architecture`
(hosted in `.github/workflows/ci.yml`).
