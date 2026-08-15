# Migration and release rehearsal

Compatibility routing lives in
[compatibility-governance.md](../implementation/compatibility-governance.md).
The adopter-facing 1.0 SemVer promise is
[1.0-compatibility-policy.md](../implementation/1.0-compatibility-policy.md)
(approved by `COMP-1.0-D-contract-freeze-00b78667ecc4`). Historical
tagged `0.1.0` scope stays in
[preview-compatibility-policy.md](../implementation/preview-compatibility-policy.md).
The family table is
[1.0-compatibility-matrix.md](../implementation/1.0-compatibility-matrix.md).

This tree carries lockstep **`1.0.0`**. Local tag `v1.0.0` exists.
The last pushed GitHub tag is `v0.1.0`. Do not treat staged artifacts
as a crates.io / PyPI / npm publication. Support windows are in
[support.md](support.md). Published conformance suites are in
[conformance.md](conformance.md).

## Supported 0.1.0 → 1.0.0 paths

Supported means in-tree starters. No external adopter is named in the
Phase 9 entrance review or the PR-066 soak note.

| Project | Path |
| --- | --- |
| `examples/rust-minimal` | Stay on lockstep crates at `1.0.0`. No journal/WIT/AgentSpec rewrite. |
| `examples/python-minimal/*` | Pin `finstack-ai==1.0.0`. `import finstack_ai` public names stay; see the Python export baseline. |
| `examples/durable-interaction` | Journal candidate-v1 is the 1.0 durable line. No meaning change. Unknown state-bearing fields stay fatal. |
| `examples/browser-minimal` | Uses experimental IndexedDB. **Excluded** from the permanent promise; inspect-not-continue. |
| `examples/ts-alpha-install` | JS/WASM exports stay; SharedArrayBuffer stays post-preview. Consume from git until npm publishes. |
| Plugin templates / reference guests | `@0.0.4` stays loadable and experimental. Retarget to `@1.0.0` with [guest MIGRATION.md](../../plugins/finstack-ai-guest-sdk/MIGRATION.md). |

### Per-surface notes

- **Journal / snapshots:** candidate-v1 is the 1.0 durable line.
  Meaning breaks still need an ADR and a migration.
- **AgentSpec / locks:** strict reject-unknown. Additive fields need a
  version or default.
- **WIT guests:** `@0.x` → `@1.0.0` is a documented retarget, not a
  silent rewrite.
- **Remote protocol:** inbound reject-unknown; version handshake.
- **Process protocol:** handshake-only. Session vocabulary is later and
  is not frozen.

### Offline converters

```text
uv run --no-project python tools/migrate/migrate.py agentspec path/to/agent.json --dry-run
uv run --no-project python tools/migrate/migrate.py journal path/to/records.jsonl --dry-run
uv run --no-project python tools/migrate/migrate.py manifest path/to/plugin.manifest.json --out path/to/plugin.manifest.json
mise run migrate -- agentspec path/to/agent.json --dry-run
```

Converters fail closed on unknown state-bearing fields. They are not a
hosted migration service and not a Cargo xtask.

Experimental-only projects (IndexedDB, `@0.x` WIT after freeze, process
session vocabulary) are labeled exclusions, not a permanent migration
promise.

## Release rehearsal

Commands are recorded in
[release-rehearsal.md](../implementation/release-rehearsal.md).

```text
mise run release-rehearsal
```

Two local staging runs from the same commit must produce identical
checksums for the existing staging paths (Python wheel/sdist, npm tarball
when the WASM package is already generated, crate package lists). The
**last pushed GitHub tag** is `v0.1.0`. Local tag `v1.0.0` exists and
is not pushed. This rehearsal does not publish to registries.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
