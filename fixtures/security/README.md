# Security fixtures

| Path | Purpose | Evidence eligible |
| --- | --- | --- |
| `canary-redaction/` | Contract-only redaction marker fragments; secret-scan negatives use runtime-assembled temporary canaries | No (`evidence_eligible = false`) |

Do not commit credential-shaped material under `examples/` or `fixtures/`.
Secret-scan negative proof is owned by the temporary-repo canaries
assembled from `canary-redaction/`; there is no mise wrapper.
