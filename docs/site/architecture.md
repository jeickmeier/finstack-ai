# Native architecture

```mermaid
flowchart LR
    App["Rust application"] --> SDK["finstack-ai SDK"]
    SDK --> Lock["AgentSpec + exact lock"]
    SDK --> Runtime["bounded native runtime"]
    Runtime --> Kernel["deterministic I/O-free kernel"]
    Runtime --> Model["OpenAI-compatible Model"]
    Runtime --> Tools["Calculator / scoped filesystem Toolsets"]
    Runtime --> Store["JournalStore"]
    Model --> Endpoint["explicit HTTPS or loopback endpoint"]
    Tools --> Policy["host policy + approval floor"]
```

The kernel owns semantic state, records, events, effects, and invariants. The
runtime owns the six primary port contracts and executes effects only after the
request records commit. The SDK owns composition and retains direct resolved
handles; the per-run path performs no registry lookup.

Side-effecting tools receive a durable approval requirement in the preview
facade. The filesystem toolset holds an already-opened explicit root, rejects
symlink/path escapes, protects sensitive names, applies fixed bounds, and fails
closed on platforms without the required safe primitives.
