# PR-062 readiness pack

Status: **Approved** by `COMP-1.0-D-contract-freeze-00b78667ecc4`

This pack asked a named maintainer to approve
[`docs/implementation/1.0-compatibility-policy.md`](../../1.0-compatibility-policy.md).
That approval is recorded in `compat-1.0-decision.txt`.

Do not write `G8-D-*`.
Do not cut or publish `1.0.0`.

## What is ready

- Freeze set and experimental exclusions are written
- Leaf-versioning policy keeps lockstep (no coupling-harm evidence)
- Deprecation inventory is empty of first-party aliases and states the
  `since` + removal rule
- WIT `@1.0.0` worlds sit beside `@0.0.4`; in-process and isolated
  host adapters accept both majors
- Breaking-change negatives exist for every frozen §6.1 family
- Offline converters live under `tools/migrate/`
- TM-08 review is recorded in `tm-08-review.md`
- Named maintainer approval of the 1.0 policy

## What is not claimed

- G8
- Registry publication
- External adopter soak of the `1.0.0` migration path
- Successful isolated instantiate of a rebuilt `@1.0.0` `component.wasm`
