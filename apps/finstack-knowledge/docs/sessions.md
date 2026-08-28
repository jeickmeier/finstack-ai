# Sessions, lanes, and the event stream

A **session** is a durable conversation owned by a journal store
(`Session::create` / `Session::open` over any `JournalStore`). Sessions
contain **lanes** — independent ordered histories; the default lane is
`main`. The knowledge agent keeps one sqlite journal per data directory
(`<data_dir>/journal.sqlite3`), so the CLI and Python notebooks can open
the *same* sessions from either surface.

Running on a lane: `Lane::run(&agent, request)` returns an `AgentRun`.
Drive it with `AgentRun::next_event_batch()` and finish with the run
result. The batches carry `RunEvent`s whose `RunEventKind`s are the
cross-surface contract: a CLI text renderer, an NDJSON stream, a Python
`EventBatchIterator`, and the browser example all consume the same kinds
in the same order.

Useful operations:

- `JournalStore::scan` pages over sessions (the CLI's `sessions list`).
- `Lane::inspect` returns the lane's history for rendering
  (`sessions show`).
- `JournalStore::write_metadata` attaches names with compare-and-swap
  semantics (`sessions name`).
- `Lane::suspend` / `Lane::resume` park and recover an in-flight run.
- `AgentRun::cancel` settles a run as cancelled; the journal keeps the
  prefix that actually happened.

Interactions (elicitation requests raised by tools) surface as events;
resolve them with `AgentRun::resolve_interaction`.
