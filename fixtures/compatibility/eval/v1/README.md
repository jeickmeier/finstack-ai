# Evaluation v1 fixtures

`valid--spec-minimal.json` is consumed by configuration and memory/SQLite store
conformance tests. Store schema v1 uses a dedicated database (`user_version=1`)
and append-only mutation rows. Attempt reservations precede preparation;
execution identities bind the actual session/lane before dispatch. An
indeterminate attempt cannot admit a replacement until reconciliation proves no
admission. Final subject results remain authoritative when scoring fails.
