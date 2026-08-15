# Security advisories

This index is the public advisory surface required by Threat Model §14
and PR-064-A04. Report vulnerabilities privately to
`me@jeickmeier.com` as described in [SECURITY.md](../../../SECURITY.md).
Do not open a public GitHub issue for a vulnerability that could harm
users or deployments.

No GHSA or CVE identifier exists for this project. This file does not
invent one.

## Process

1. Triage against the [SECURITY.md](../../../SECURITY.md) severity rubric.
2. Record the finding in the active review register when the work is
   in-tree (for PR-064: [FIND-064-*](../../implementation/artifacts/pr-064/findings.md)).
3. When a public identifier is assigned, add a row below with the
   identifier, affected versions, patched versions, and a link to the
   advisory text. Keep payloads, credentials, and exploit proof-of-concept
   out of this index.
4. Revoke compromised publishing credentials, plugin trust roots, and
   published artifacts when those surfaces exist. crates.io / PyPI / npm
   publication is not yet exercised.

## Index

| Identifier | Severity | Affected | Patched | Advisory |
| --- | --- | --- | --- | --- |
| — | — | — | — | No published advisory |

The empty row is intentional. An empty index is allowed while no
CVE/GHSA exists.
