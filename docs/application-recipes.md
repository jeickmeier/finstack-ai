# Application recipes

These recipes run offline, without credentials, against real SDK execution.
Use `mise run test-recipes-rust` and, after building the Python binding,
`mise run test-recipes-python`. CI executes both sets. Each command bounds
execution, owns temporary storage, checks results, and fails on lifecycle errors.

| Application | Rust command | Python command | Checked behavior |
|---|---|---|---|
| Persistent knowledge assistant | `cargo run -p finstack-ai-knowledge --example persistent` | `uv run python examples/python-notebooks/persistent_assistant.py` | SQLite session reopening, durable document bytes, validated structured results, event streaming, cancellation, bounded search maintenance |
| Approval workflow | `cargo run -p finstack-ai-example-durable-interaction` | `uv run python examples/python-notebooks/durable_approval.py` | Process termination at approval, fresh-host recovery, one protected write, subsequent model completion, Accepted delivery, inbox/wake cleanup |
| Supervisor | `cargo run -p finstack-ai-native-examples --bin supervisor` | `uv run python examples/python-notebooks/supervisor.py` | Two distinct specialists, isolated child sessions, collected results, committed lineage, parent cancellation |

## Component selection

The persistent recipe uses the existing knowledge application definition in
`apps/finstack-knowledge` and its shared Python notebook composition. Its journal,
artifact store, memory, document ingestion, instructions, and compaction remain
application components. Retain the returned `KnowledgeAgent`, use its `.agent`
for SDK calls, and run `.search.maintain()` (Rust) or `.maintain()` (Python) after
settled turns. Sources bind authorized sessions at composition; create a new
session before binding it. The example supplies an offline model and an answer
schema. Rust uses the released Ollama provider against a bounded loopback HTTP
fixture; Python uses `PythonModel` callbacks. Select a live provider in the same
knowledge configuration when deploying. Provider and media-tool credentials
belong to their respective component configurations.

The approval recipe uses `DurableHost` with a SQLite worker/journal store and a
tool whose required approval and at-most-once retry policy describe its file
mutation. An explicit decision is durably buffered before the worker resumes.
The recorded delivery status is `Accepted`; completion and cleanup are separate
assertions. The Rust recipe also checks sweeping an abandoned interaction.

The supervisor creates exactly two children and sets a maximum depth of one.
Each run allows one model cycle and has a deadline. Child depth is not a count
limit: the host owns the fixed fan-out count. Specialists cannot create further
children. The host collects results and passes the computed summary to the
parent's offline model fixture. Committed `RunAccepted` relations retain parent,
root, effect, and depth identity; the Rust recipe additionally checks both
`ChildRunPrepared` records. Applications can use the same host orchestration with
live providers and explicit request inputs for a subsequent synthesis turn.

## Lifecycle ownership

Retain the persistent data directory in a deployed application. Open the same
journal, artifacts directory, tenant and session identity on restart, then run on
that session's lane. A second call to `Agent.start` creates a new session; it does
not continue the opened conversation. Output schemas are compiled once by the
Rust validator and supplied to native providers through framework schema metadata.

A run has one event consumer. Drain it concurrently with waiting for the result,
or explicitly close events when using APIs intended only to return a result.
Dropping a run or an await detaches observation; explicitly cancel and await the
terminal outcome when the application owns cancellation. Join the event consumer
and fixture tasks before leaving the scope. Python callbacks execute on a
separate event loop; use thread-safe futures/signals to communicate with the
application loop, as the supervisor recipe does.

For durable work, register the same workflow kind and resolved application
configuration in each process. `start` admits work; `tick` leases and drives it to
its next durable wait or terminal state. Tick while the host is active, expose
pending interactions through the application's authenticated decision surface,
and call `shutdown` before releasing local resources. The parked-process recipe
intentionally skips shutdown to prove recovery without a retained controller.
An in-process supervisor owns its live children; the recipe does not claim to
reconstruct host orchestration after a process crash.

## Failure handling and deployment

Surface configuration drift, missing artifacts, deadline cancellation, provider
errors, and unresolved external effects as distinct failures. Re-registering a
different output schema is configuration drift. A non-idempotent tool that was
dispatched without a committed receipt requires reconciliation; restarting does
not authorize repeating it. Lease loss stops and joins local driving but cannot
reverse a dispatched external operation.

The offline fixtures assert actual requests, committed history and terminal
outcomes. They are deployment examples, not network services. Embed the host in
your application's worker and supply credentials at process construction; keep
journal and artifact paths persistent and scoped to the intended tenant. The
remote server remains a reference implementation. Production authentication,
request routing and deployment control planes belong to the consuming
application and are outside these recipes.
