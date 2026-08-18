# Release engineering

Operational register for multi-package recreate, checksums, SBOMs,
provenance, signing procedure, nightly/canary definitions, and
rollback/hotfix. Owner: `me@jeickmeier.com`.

This file does not publish, yank, push a tag, or record `G8-D-*`.
Workspace version is `1.0.0`. Local tag `v1.0.0` exists. Canary
label default is `1.0.0-canary`.

## Local commands

| Artifact | Command |
| --- | --- |
| Two-run recreate (A01) | `uv run --no-project python tools/docs/recreate_release.py` |
| Starter RC (A02) | `uv run --no-project python tools/docs/starter_rc.py` |
| Published conformance (A03) | `cargo test -p finstack-ai-test --locked --lib -- conformance::ports::tests` |
| Hotfix pair (A04) | `uv run --no-project python tools/docs/hotfix_rehearsal.py` |
| Advisories / licenses / sources | `cargo-deny check` (also via `mise run ci-all`) |
| npm stage | `mise run build-wasm -- release` then `uv run --no-project python tools/wasm_package/stage.py` |
| WIT | `uv run --no-project python tools/wit_bindgen/generate.py --check` |

`SOURCE_DATE_EPOCH=0` is the reproducibility pin (WASM and staging).

Two clean staging runs from the **same commit** must produce
byte-identical SHA-256 sets for crate package lists, wheels/sdist,
npm stage (when `dist/` exists), WIT packages, and SBOMs. Recreate
from the tagged local `v1.0.0` commit
`6e9ec39fae89a70f696ee740de2d2094670cba3e`. Do not `git tag` here.

## GA tag procedure (PR-066 only)

Local annotated tag `v1.0.0` was cut at
`6e9ec39fae89a70f696ee740de2d2094670cba3e` under the owner G8
sentence. Tag push, registry publish, GitHub Release, announce, and
`release/1.0` remain separately named.

1. Land the release commit on the authorized target.
2. Recreate: `uv run --no-project python tools/docs/recreate_release.py`. Retain
   `docs/implementation/artifacts/pr-066/run-a.SHA256SUMS`.
3. `git tag -a v1.0.0 <commit>` (executed locally; not pushed).
4. Publish crates, wheels, npm, and WIT only after a later sentence
   names registry credentials.
5. Cut `release/1.0` from that tagged commit (separately named).

Tag `v0.1.0` is already `9d09b87108f6286918fcc436c53d195d0b6b10cc`.

## Signing and provenance

Required locally: SHA-256 over staged bytes plus a provenance
statement naming the commit, `mise.toml` pins, version `1.0.0`,
and `staged_not_published: true`.

Hosted keyless / Sigstore / OIDC is the existing
`.github/workflows/npm-release-staging.yml` `id-token: write`
sketch. Dispatching it or publishing signatures to a public log is
a **separately named** external action. Do not run it from this PR.

## Nightly and canary

`.github/workflows/nightly.yml` is schedule + `workflow_dispatch`.
It builds canary staging (`0.1.0-canary`) and runs
`uv run --no-project python tools/docs/starter_rc.py`. Dispatching hosted nightly is a separately
named external action. Canary artifacts stay unpublished and must
not claim `1.0.0`.

## Rollback and hotfix

Rollback means consumers stay pinned to baseline checksum set B.
Maintainers stage patch-line set H and, after a named publish
action, release H as the next patch. Do not rewrite published
bytes.

Rehearsal: `uv run --no-project python tools/docs/hotfix_rehearsal.py` writes `SHA256SUMS-B` and
`SHA256SUMS-H` under `docs/implementation/artifacts/pr-065/` and
asserts B ≠ H while a documented pin of B still verifies.

### Yank / retract (document only; do not run)

```text
# crates.io — do not run under this envelope
cargo yank --vers 0.1.0 finstack-ai

# PyPI — do not run; use the warehouse yank/retract UI for the
# exact filename whose SHA-256 is in SHA256SUMS-B.

# npm — do not run
npm deprecate @finstack/ai@0.1.0 "retracted; pin the previous checksum"
```

## TM-18 review

Threat Model §18 (dependency or release compromise) is reviewed
for this PR's release automation, checksums, SBOM, cargo-deny
restore, and rollback procedure.

| Control | Evidence |
| --- | --- |
| Dependency advisories/licenses/sources checked continuously | Restored `deny.toml` + `cargo-deny check` via `mise run ci-all` (closes FIND-064-004) |
| SBOM, checksums, provenance | `uv run --no-project python tools/docs/recreate_release.py` |
| Signatures where the ecosystem supports them | Procedure only; hosted Sigstore stays separately named |
| Reproducible release | Two-run SHA-256 identity; `SOURCE_DATE_EPOCH=0` |
| Rollback / hotfix | `uv run --no-project python tools/docs/hotfix_rehearsal.py`; yank commands documented not run |
| Least-privilege release credentials | No registry tokens in this tree; workflows use `contents: read` |

This review is not `G8-D-*`. G8 later passed via
`G8-D-general-availability-a889a29a3f54`. FIND-064-010 (unpublished
registries) stays Accepted until the first named publish. FIND-064-003
(unbounded settlement map) is Closed: the replica fails closed at a
receipt cap and indexes by `command_id`.

## Related

- [release-rehearsal.md](release-rehearsal.md)
- [support-windows.md](support-windows.md)
- [artifacts/pr-065/](artifacts/pr-065/)
