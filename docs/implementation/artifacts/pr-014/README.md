# PR-014 evidence

PR-014 implements the runtime commit loop and bounded in-memory journal on
`codex/pr-014-runtime-commit-loop`.

The reviewed implementation is split into four coherent commits:

- `dfe42cf` adds the external-command kernel contracts, state-neutral durable
  rejection record, and candidate-v1 compatibility fixtures;
- `25f921d` adds the target-correct `JournalStore` port and explicitly
  non-durable bounded memory-store leaf;
- `450a73b` adds the authoritative commit coordinator and bounded Tokio task
  owner; and
- `ff9178f2244c8d8fd0c62cbba18b2b4dfbee2a29` adds direct-locator external
  routing and the fail-closed security-audit path.

The exact local command/result inventory is in
[`candidate-validation.txt`](candidate-validation.txt). The TM-04/TM-10/
TM-11/TM-12/TM-14/TM-16/TM-18 and SEC-INV disposition is in
[`security-review.txt`](security-review.txt).

Candidate acceptance evidence is complete for PR-014's local implementation
scope. Hosted Linux/macOS/Windows, coverage, fuzz-smoke, nightly, security,
schema-governance, workflow, and release evidence plus immutable merge-tree
verification are recorded in [`hosted-validation.txt`](hosted-validation.txt).
Pull request [#8](https://github.com/jeickmeier/finstack-ai/pull/8) merged as
`399f3a7d9d987268f4d79ab90f31b93f854084f8`; its tree is identical to the
validated head `8a9f9a6948dc6dc72a84937dda452915a666257f`. PR-014 is `Done`.

Successful provider model-response decoding remains PR-015, successful
interaction resolution remains PR-044, and canonical CBOR/checksum plus
persistent stores remain PR-039 onward.

[`SHA256SUMS`](SHA256SUMS) binds the candidate validation, security review, and
hosted/merge evidence artifacts.
