# Guest world version negotiation

Guests pin manifest `version` `0.0.4` and worlds `toolset-plugin` or
`context-plugin`. The host is exact-world: an unknown world or a
`@1.0.0` package version fails closed as `plugin_registration_invalid`.

Linking `finstack:ai-host` logging and blobs is not ambient WASI,
filesystem, network, clock, environment, or secret access.

WASM guests do not speak the process-protocol hello. FR-PLG-004 version
negotiation remains the later process adapter.

## Hypothetical `@0.0.4` → `@0.0.5` retarget

This repository does not ship `@0.0.5`. If a later breaking pre-1.0
change increments the WIT package:

1. Add `finstack:ai-toolset@0.0.5` (or context) beside `@0.0.4`.
2. Add the new world name to host `ALLOWED_WORLDS`.
3. Point the guest `wit/` vendor and `toolset_plugin!` path at `@0.0.5`.
4. Rebuild and encode the component.
5. Keep historical `@0.0.4` fixtures under
   `fixtures/compatibility/wit/v0.0.4/`.

A host that still lists `@0.0.4` continues to load existing `@0.0.4`
components. Silent semantic discard of unknown authorization,
idempotency, or catalog fields is prohibited. See ADR-035.
