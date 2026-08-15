# Plugins

WIT/Wasmtime isolation is optional. Worlds are experimental `@0.0.4`.
`@1.0.0` worlds stay blocked until PR-062.

In-process WIT guests inherit host authority and are not a sandbox. The
isolated host (`finstack-ai-plugin-host`) is [T3](security-trust-levels.md):
deny-by-default WASI, fuel/limits, and signature policy.

## Quick start

```text
mise run check-plugin-template
```

Copy `plugins/templates/toolset-plugin/` or
`plugins/templates/context-plugin/`. Depend on `finstack-ai-guest-sdk` only.
See [plugins/README.md](../../plugins/README.md).

The default SDK bundle does not depend on Wasmtime.

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
