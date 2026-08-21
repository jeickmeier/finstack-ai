# golden-trace fixtures

Corpus for scripted-input and golden-trace schemas. Unknown fields and
oversized payload declarations are rejected.

The trace corpus executes successful model completion, chunk-split invariance,
deferred external completion (including present empty text) under the original
effect, and `before_finalize` continuation through the real Rust reducer
adapter. Expected values use the approved typed path-aware canonical
fingerprints and are checked-in observations; tests never rewrite them.

Tool-batch semantic snapshots live under `v1/tool-batch/`. Kernel
reducer tests execute them through real `decide` and committed `apply`, compare
source-ordered records/messages/events, and replay every committed batch to
prove the live and replayed schema-2 state hashes are equal.

The strict `v1/test-kit/` scenario suite covers child lineage,
deferred external completion, typed interactions, duplicate completion,
before-finalize continuation, and replay-safe compaction. Public test-kit APIs
load the suite and bind each scenario to stable diagnostic contract labels.

See `v1/scripted-input/`, `v1/trace/`, and `v1/test-kit/`.
