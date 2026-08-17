# Toolsets

A first-party toolset stays small: business logic aside, the registration
and policy surface is intended to stay under 100 lines (NFR-DX-002).

| Crate | Role |
| --- | --- |
| `finstack-ai-tools-calculator` | Bounded read-only arithmetic |
| `finstack-ai-tools-filesystem` | Capability-scoped root; no symlink escape |
| `finstack-ai-tools-shell` | Deny-by-default argv, empty env, timeout |

The `coding` binary in [rust-minimal](../../examples/rust-minimal/README.md)
composes those leaves with repository/memory context, sliding-window
compaction, and a `before_finalize` verifier over a keyless loopback model.
The toolset, context, and sliding-window compaction legs work. Summarize
compaction stays unlandable — see
[why summarize compaction cannot complete](middleware.md#why-summarize-compaction-cannot-complete).

Native toolsets are [T1](security-trust-levels.md). They inherit process
authority. Do not call them a sandbox. Untrusted code belongs on the
[plugin](plugin.md) path.

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
