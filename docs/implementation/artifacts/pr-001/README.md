# PR-001 validation artifacts

Local acceptance evidence captured against immutable commit `73bfe88c8dbc92c4e4c6eba1a5a7814240c2e862` on 2026-08-08.

| File | Criterion | Notes |
| --- | --- | --- |
| `environment.txt` | all | Host toolchain snapshot |
| `cargo-metadata.json` | A01 | Full `cargo metadata --format-version 1` |
| `dependency-direction.txt` | A01 | Workspace edge summary + pass assertion |
| `cargo-check.txt` | A02 | `cargo check --workspace --all-targets` and `cargo check -p finstack-ai-kernel` |
| `kernel-deps.txt` | A03 | Kernel dependency inventory + forbidden-set check |
| `mise-install.txt` / `mise-doctor.txt` | A04 | Documented bootstrap commands |
| `ownership-security-review.txt` | A05, A06 | Manual review of governance/security files |
| `SHA256SUMS` | all | Digests of the files above |

See the [evidence register](../../evidence-register.md) for stable evidence IDs and acceptance dispositions.
