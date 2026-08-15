# Trust levels

This page is the single T0–T5 matrix. Every starter and guide that
registers an extension links here. In-process code is never a sandbox
(TM-06). T1 and T2 are never called isolated.

| Class | Meaning | Starter label |
| --- | --- | --- |
| T0 | Kernel; I/O-free | not a user extension |
| T1 | Native Rust in-process | rust-minimal, native providers/tools, SQLite, local workflow driver |
| T2 | Python/JS callbacks | python-callback, JS host adapters, Python service starter |
| T3 | WIT/Wasmtime or process | plugin templates; process family is handshake-only |
| T4 | Remote principal | server clients; external completion |
| T5 | Content | prompts, retrieval, artifacts, journal payloads |

## What isolation is

Only the optional Wasmtime host (`finstack-ai-plugin-host`) is an isolation
boundary, and only for guests loaded through that host with deny-by-default
WASI. In-process WIT guests, native leaves, Python callbacks, and JS host
objects inherit process or page authority.

## Related

- [Deployment gates](security-deployment.md)
- [Provider security](provider-security.md)
- [SECURITY.md](../../SECURITY.md)
- [Threat Model G7 review](../implementation/threat-model-g7-review.md)
