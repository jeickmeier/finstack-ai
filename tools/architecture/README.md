# Architecture enforcement (PR-002)

Makes the microkernel boundary mechanically enforceable.

## Commands

```bash
mise run architecture          # pytest + policy check + wasm binding target check
mise run test-architecture     # Python tests only (pytest)
mise run lint-architecture     # Ruff lint
mise run format-architecture   # Ruff format check
uv run --no-project python tools/architecture/check.py
```

Hosted CI invokes `mise run architecture` from [`.github/workflows/ci.yml`](../../.github/workflows/ci.yml).
Treat a nonzero exit as a failing merge gate.

## Layout

| Path | Role |
| --- | --- |
| `check.py` | Policy checker entry point (`cargo metadata` + source scans) |
| `policy.toml` | Package roles, edges, forbidden deps, feature rules |
| `allowlist.toml` | Approved, ADR-backed suppressions (empty by default) |
| `tests/` | Unit tests, including synthetic forbidden-dep graphs |

Fixtures live under [`fixtures/architecture/`](../../fixtures/architecture/).

## Check IDs

| ID | Governing rules | Meaning |
| --- | --- | --- |
| ARCH001 | ENG-ARCH-001/003/006 | Forbidden kernel dependency or kernel I/O import |
| ARCH002 | ENG-ARCH-002 | Reserved for six-port / seventh-port ownership (compile proofs deferred to PR-014–PR-018) |
| ARCH003 | ENG-ARCH-003 | Concrete provider/tool/store imports in kernel/runtime |
| ARCH004 | ENG-ARCH-003 | Illegal workspace dependency edge |
| ARCH005 | ENG-ARCH-003 | Central concrete provider/tool match registry |
| ARCH006 | ENG-ARCH-005 | Per-token host callback surface |
| ARCH007 | ENG-ARCH-001, ENG-SEM-002 | Ambient kernel time/randomness |
| ARCH008 | Architecture §22.2 | Unbounded channel constructors |
| ARCH009 | Architecture §22.2 | Public task-local/thread-local request context |
| ARCH010 | Eng Standards §14 | Incomplete/invalid architecture exception |
| ARCH011 | Eng Standards §3.2 | Runtime/facade/WASM feature separation |
| ARCH012 | Eng Standards §3.2 | Tokio/native-I/O in browser WASM graph (metadata rooted at `finstack-ai-wasm`) |

Semantic state-machine lint (`ENG-SEM-*` transition rules) is intentionally out
of scope for PR-002 and arrives with the kernel.

## Allowlist / exceptions

1. Open an ADR when required by Implementation Plan §6.3.
2. Add a complete row to [`docs/implementation/exceptions-register.md`](../../docs/implementation/exceptions-register.md).
3. Mirror it in `allowlist.toml` with the same `exception_id`, exact `subject`, and `adr`.
4. `ARCH001` and `ARCH002` are non-waivable.

## CI

Hosted jobs invoke the same task name:

```bash
mise run architecture
```

Do not reimplement the policy in workflow YAML. See [`.github/ci/README.md`](../../.github/ci/README.md).
