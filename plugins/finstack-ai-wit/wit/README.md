# wit (reserved)

Canonical future WIT package roots live under `wit/v<x.y.z>/` as separate
single-package directories (for example `wit/v0.1.0/`). PR-004 reserves the root
only; no `.wit` payloads are created here.

Fixtures must use the same semver segment:
`fixtures/compatibility/wit/v<x.y.z>/<kind>/<valid|invalid|roundtrip>--<slug>.<ext>`.
