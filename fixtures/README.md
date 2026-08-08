Fixture trees used by validation, architecture enforcement, and CI evidence.

- `architecture/` — PR-002 dependency, source-guard, and waiver case files.
  See [`architecture/README.md`](architecture/README.md).
- `ci/` — private release-smoke binary for PR-003. See [`ci/README.md`](ci/README.md).
- `security/` — contract-only canary-redaction fragments (non-evidentiary) and
  secret-scan negative ownership. See [`security/README.md`](security/README.md).
- `compatibility/` — PR-004 reserved roots and naming conventions for versioned
  contract fixtures. See [`compatibility/README.md`](compatibility/README.md).

Do not commit credential-shaped material under `examples/` or `fixtures/`.
Secret-scan negatives are assembled only in temporary repositories by
`mise run secret-scan-canary`.

Golden-trace and benchmark-report fixtures land under `compatibility/`
(PR-005). Crash-prefix and port compile fixtures land in later pull requests.
See docs/planning/03-finstack-ai-technical-design.md §2.
