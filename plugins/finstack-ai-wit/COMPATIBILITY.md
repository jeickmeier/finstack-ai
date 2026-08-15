# Experimental WIT compatibility

Family: `wit`  
Profile: `exact-world`  
Stability: `experimental-0.x`  
Packages: `finstack:ai-types@0.0.4`, `finstack:ai-host@0.0.4`,
`finstack:ai-toolset@0.0.4`, `finstack:ai-context@0.0.4`

## Promise

Worlds are exact. Unknown fields, unknown enums, omitted security
metadata, and undeclared exports are rejected. There is no permissive
default for `execution-mode`, `side-effect`, `retry-safety`,
`approval-policy-json`, or `max-result-bytes`.

The workspace crate version is staged unpublished `0.0.4`. Plugin alpha
publishes `@0.x` packages only. `@1.0.0` worlds are generated only at the
framework 1.0 compatibility gate. `mise run gen-wit` fails if a `@1.0.0`
package appears.

## Deprecation

A later breaking pre-1.0 change increments the package/world version and
requires adapters plus fixtures in the same change. Historical `@0.0.4`
fixtures stay under `fixtures/compatibility/wit/v0.0.4/`. Silent
semantic discard of unknown authorization, idempotency, or catalog
fields is prohibited.

## Surface

`toolset-plugin` exports only `toolset` (`list-tools`, `call`).
`context-plugin` exports only `context-provider` (`collect`). Host
imports grant only `logging.log` and `blobs.read`. Linking those
interfaces is not ambient WASI, filesystem, network, clock, environment,
or secret access. The worlds cannot invoke a nested agent or persist a
competing run lineage. Initialize, health, shutdown, and warmup are
host-side hooks, not WIT exports. Plugin manifests are host types.

`call-context` is a sanitized identity/scope projection. It never
carries credentials, full claims, cancellation, or attempt counters.

In-process `finstack-ai-wit` adapters inherit host authority. Isolated
Wasmtime instantiation is the `finstack-ai-plugin-host` leaf.

Guest authors should depend on `finstack-ai-guest-sdk` and the vendored
`@0.0.4` copies it ships. Do not depend on this crate or on
`finstack-ai-plugin-host` from a guest `Cargo.toml`. See
[`../finstack-ai-guest-sdk/README.md`](../finstack-ai-guest-sdk/README.md)
and [`../finstack-ai-guest-sdk/MIGRATION.md`](../finstack-ai-guest-sdk/MIGRATION.md).

## Payload ceilings

TDD §6.5 ceilings are enforced before allocation:

- 4 MiB individual text or byte string
- 1 MiB `RawJson` / catalog JSON / args-json / result JSON
- 64 KiB metadata
- `max-result-bytes == 0` is invalid
