# Migration and release rehearsal

Compatibility routing lives in
[compatibility-governance.md](../implementation/compatibility-governance.md).
Open public-surface deltas before `0.1.0` are triaged in
[public-api-change-backlog.md](../implementation/public-api-change-backlog.md).
That backlog is Phase 8 entrance evidence, not a published preview policy.

Published preview compatibility policy is PR-061. This tree stays on
workspace **0.0.4 unpublished**. Do not treat staged artifacts as `0.1.0`.

## Release rehearsal

Commands are recorded in
[release-rehearsal.md](../implementation/release-rehearsal.md).

```text
mise run release-rehearsal
```

Two local staging runs from the same commit must produce identical
checksums for the existing staging paths (Python wheel/sdist, npm tarball
when the WASM package is already generated, crate package lists). The
**public tag** is PR-061. This rehearsal does not `git tag` or publish.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
