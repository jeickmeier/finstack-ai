# Python guide

Install the staged `finstack-ai` wheel. Users do not need a Rust toolchain
when they consume a built artifact.

Workspace version is **0.0.4 unpublished**. The package is not on PyPI.

## Quick start

Build the local candidate, then run a starter against that wheel:

```bash
uv build --project bindings/finstack-ai-python --out-dir target/python-dist
uv run --isolated --no-project --with target/python-dist/*.whl \
  python examples/python-minimal/python-callback/main.py
```

From a repository checkout, the same offline callback path also works with
an editable install:

```bash
uv run --isolated --no-project --with-editable bindings/finstack-ai-python \
  python examples/python-minimal/python-callback/main.py
```

## Two constructions

| Starter | Trust | Network |
| --- | --- | --- |
| [python-callback](../../examples/python-minimal/python-callback/README.md) | [T2](security-trust-levels.md) trusted in-process callback | None |
| [rust-backed](../../examples/python-minimal/rust-backed/README.md) | [T1](security-trust-levels.md) curated Rust provider | Default is construct-only; `--run` needs an explicit endpoint |
| [service](../../examples/python-minimal/service/README.md) | T2 callback | Resolve once, health, one offline request |

`import finstack_ai` does not create a provider, start Tokio, or read
credentials. `Agent.openai_compatible()`, `Agent.anthropic()`, and
`Agent.ollama()` are explicit. Lazy `finstack_ai.providers.*` stay unloaded
until attribute access.

Never put secrets in `AgentSpec`. See [provider security](provider-security.md).

## License and governance

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[DCO](../../CONTRIBUTING.md). [Maintainers](../../GOVERNANCE.md).
[ADRs](../implementation/adr-register.md). [RFCs](../rfcs/README.md).
