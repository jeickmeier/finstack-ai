# Toolsets

Trusted native Toolset leaves. They implement the public
`finstack-ai-runtime` `Toolset` port in-process ([T1](../../docs/site/security-trust-levels.md)).
They are not a sandbox.

| Crate | Role |
| --- | --- |
| [`finstack-ai-tools-calculator`](finstack-ai-tools-calculator/README.md) | Bounded arithmetic; no I/O |
| [`finstack-ai-tools-filesystem`](finstack-ai-tools-filesystem/README.md) | Explicit opened root; rejects path escape |
| [`finstack-ai-tools-shell`](finstack-ai-tools-shell/README.md) | Bounded process execution |

Side-effecting tools receive a durable approval requirement in the SDK
facade. Isolated WIT guests live under [`../../plugins/`](../../plugins/),
not here.
