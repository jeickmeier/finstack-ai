# Guest world version negotiation

Guests may pin manifest `version` `0.0.4` or `1.0.0` and worlds
`toolset-plugin` or `context-plugin`. The host is exact-world: an unknown
world or an undeclared field fails closed as
`plugin_registration_invalid`. Runtime discovery is lockfile-only; the
host does not search a registry.

`@0.0.4` remains loadable after the 1.0 freeze. It is **experimental**
and is not the permanent WIT promise. `@1.0.0` is the frozen exact-world
line: one coarse completion, no progress batch, no resource streams.

Linking `finstack:ai-host` logging and blobs is not ambient WASI,
filesystem, network, clock, environment, or secret access.

WASM guests do not speak the process-protocol hello. FR-PLG-004 version
negotiation remains the later process adapter.

## `@0.0.4` → `@1.0.0` retarget

Do not silently rewrite published `@0.x` fixtures or compiled
components. Retarget a guest project as follows:

1. Copy `plugins/finstack-ai-wit/wit/v1.0.0/` into the crate `wit/deps/`
   (or point the vendor path at those packages).
2. Change `resolve.wit` package imports from `@0.0.4` to `@1.0.0`.
3. Set manifest `version` to `1.0.0` and recompute the host digest:

   ```text
   uv run --no-project python tools/migrate/migrate.py manifest \
     path/to/plugin.manifest.json --out path/to/plugin.manifest.json
   ```

4. Rebuild and encode the component.
5. Keep historical `@0.0.4` fixtures under
   `fixtures/compatibility/wit/v0.0.4/`. New `@1.0.0` fixtures live under
   `fixtures/compatibility/wit/v1.0.0/`.

The isolated host links both majors. Existing `@0.0.4` components keep
loading when the manifest stays `0.0.4`. A rebuilt `@1.0.0` component
is required before a `1.0.0` manifest can instantiate. Silent semantic
discard of unknown authorization, idempotency, or catalog fields is
prohibited. See ADR-035.
