# golden-trace fixtures

Corpus for scripted-input and golden-trace schemas. Unknown fields and
oversized payload declarations are rejected (PR-005-A02/A04; TDD §28.4 / §6.5).

PR-009 traces execute successful model completion, chunk-split invariance,
deferred external completion (including present empty text) under the original
effect, and `before_finalize` continuation through the real Rust reducer
adapter. Expected values use the approved typed path-aware canonical
fingerprints and are checked-in observations; tests never rewrite them.

PR-010 tool-batch semantic snapshots live under `v1/tool-batch/`. Kernel
reducer tests execute them through real `decide` and committed `apply`, compare
source-ordered records/messages/events, and replay every committed batch to
prove the live and replayed schema-2 state hashes are equal.

See `v1/scripted-input/` and `v1/trace/`.
