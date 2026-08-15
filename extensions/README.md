# Extensions

Trusted native leaf batteries that implement runtime port contracts in-process:

- `providers/` — model providers
- `toolsets/` — tool implementations
- `stores/` — journal/store implementations
- `observers/` — read-only observability adapters
- `workflow/` — runtime-driver adapters (not a seventh port)

These packages depend inward on runtime contracts (and durable stores may also use the protocol codec). They must not reverse-depend into the kernel or create facade-to-leaf cycles.

Isolated WIT/Wasmtime packages remain under [`../plugins/`](../plugins/). Do not place trusted in-process batteries there.

See [Technical Design §2](../docs/planning/03-finstack-ai-technical-design.md).
