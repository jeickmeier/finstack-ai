# PR-062 readiness pack

Status: **READY FOR NAMED DECISION**

This pack asks a named maintainer to approve
[`docs/implementation/1.0-compatibility-policy.md`](../../1.0-compatibility-policy.md)
as `COMP-1.0-D-*`. That approval is a separate owner action.

Do not write `COMP-1.0-D-*` from this file.
Do not write `G8-D-*`.
Do not cut or publish `1.0.0`.

## What is ready

- Freeze set and experimental exclusions are written
- Leaf-versioning policy keeps lockstep (no coupling-harm evidence)
- Deprecation inventory is empty of first-party aliases and states the
  `since` + removal rule
- WIT `@1.0.0` worlds sit beside `@0.0.4`; host adapters accept both
- Breaking-change negatives exist for every frozen §6.1 family
- Offline converters live under `tools/migrate/`
- TM-08 review is recorded in `tm-08-review.md`

## What is not claimed

- Named maintainer approval of the 1.0 policy
- G8
- Registry publication
- External adopter soak of the `1.0.0` migration path
