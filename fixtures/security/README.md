# Security fixtures

| Path | Purpose | Evidence eligible |
| --- | --- | --- |
| `canary-redaction/` | Contract-only redaction marker fragments for later redactor PRs; PR-003 uses runtime-assembled temporary canaries for secret-scan negatives | No (`evidence_eligible = false`) |

Do not commit credential-shaped material under `examples/` or `fixtures/`.
Secret-scan negative proof is owned by `mise run secret-scan-canary`.
