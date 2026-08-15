# PR-062 TM-08 review

Date: 2026-08-15
Reviewer: me@jeickmeier.com
Threat: TM-08 — plugin package substitution or incompatible ABI
Trigger: Threat Model §18 (WIT compatibility policy / public-contract freeze)

This review is not `G8-D-*` and is not `COMP-1.0-D-*`.

## Controls reviewed

| Control | Disposition |
| --- | --- |
| Digest / signature verification | Unchanged. Manifest digest still covers `{identity, version, worlds}`. Signature verify stays host-side. |
| Configured trust roots | Unchanged. PR-052 policy still applies. |
| Lockfile resolution | Lockfile now accepts `0.0.4` and `1.0.0` only. Unknown majors fail closed. Paths stay relative and local. |
| Interface version negotiation | Dual-major adapters: `@0.0.4` guests keep loading; `@1.0.0` guests use the frozen worlds. Unknown world, unknown major (`2.0.0`), and undeclared fields fail closed. |
| Duplicate identity rejection | Unchanged. |
| Compiled-cache key | `abi_identity(world, version)` now includes the package major so `@0.0.4` and `@1.0.0` cannot share a cache entry. Digest, engine, and target fields stay. |

## Residual

Isolated Wasmtime now generates and links both `@0.0.4` and `@1.0.0`
worlds. Existing encoded `@0.0.4` components keep loading. A `@1.0.0`
manifest plus a `@0.0.4` `component.wasm` fails instantiate. A rebuilt
`@1.0.0` encoded guest is still not in-tree; successful instantiate of
that rebuilt artifact is not claimed. Resource-based streaming and
observer WIT worlds stay deferred (ADR-035). Delivery stays at-least-once
(ADR-013).

## Decision

No new trust boundary. TM-08 controls hold for the dual-major freeze.
No control weakening is accepted.
