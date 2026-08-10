# PR-014 candidate evidence

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
scope. The logical PR remains `In progress`: no hosted pull request, immutable
merge revision, or post-merge verification exists yet. Successful provider
model-response decoding remains PR-015, successful interaction resolution
remains PR-044, and canonical CBOR/checksum plus persistent stores remain
PR-039 onward.

[`SHA256SUMS`](SHA256SUMS) binds the candidate validation and security-review
artifacts.
