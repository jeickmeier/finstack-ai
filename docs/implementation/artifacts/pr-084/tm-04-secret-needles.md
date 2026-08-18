# PR-084 TM-04 review — kernel telemetry / secret needles

Date: 2026-08-18
Reviewer: me@jeickmeier.com
Candidate: local worktree (uncommitted; no candidate SHA)
Decision: Pass for the kernel surface reviewed in this slice. No
threat-model rewrite. No Implementation Plan §6.3 ADR trigger. Do not
infer merge or G8 re-opening.

## Scope

- `finstack-ai-kernel` durable and hash-visible surfaces that can carry
  host-supplied text: `ErrorDescriptor.safe_details`, `Diagnostic`,
  `Metadata`, `RawJson`, `ExternalHandleRef`, `RunEvent` bodies, and
  `Sensitivity::{Secret, Credential}`
- Finding #12 only. Bundle `contains_secret` / `key_looks_secret` stay
  owned by PR-067 (`artifacts/pr-067/tm-04-secret-needles.md`)

Triggered control: Threat Model TM-04 / SEC-INV-005 (secret material
must not enter ordinary records, events, errors, or telemetry).

## Disposition

PASS  The kernel is I/O-free and does not emit telemetry. Observer and
      exporter redaction remain runtime/SDK work. Kernel types classify
      sensitivity and require `safe_details` / metadata to be
      non-secret; they do not inspect values for credential strings.

PASS  `Sensitivity::Credential` is documented as never placed in
      records or events. `ErrorDescriptor.safe_details`,
      `Diagnostic.metadata`, `ArtifactRef.metadata`, and
      `ExternalHandleRef` are contracted as non-secret. No new journal
      field in this slice accepts a secret.

PASS  Human-path `RawJson` now enforces byte, member, and depth
      ceilings before full materialization. That bounds hostile
      materialization; it is not a secret scanner.

RESIDUAL  An innocuous key or `safe_details` member that holds a
      credential string is still a host problem. A kernel-side
      key-token scan would invent a new contract and is out of
      PR-084.

No journal, WIT, or protocol meaning change.
