# Python guide

Install the staged `finstack-ai` wheel. Users do not need a Rust toolchain
when they consume a built artifact.

Workspace version is **1.0.0** unpublished. The last public tag is
`v0.1.0`. The package is not on PyPI.

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
credentials. `Agent.openai()`, `Agent.anthropic()`, `Agent.ollama()`,
`Agent.openrouter()`, `Agent.gateway()`, and `Agent.e2b_sandbox()` are
the same Rust-owned constructors as WASM. wasm-host fail-closed is a
Rust platform error, not a missing method. They accept the same
keyword-only T2 ports as `Agent.from_python`. `openai` takes required
keyword-only `api_key` (Bearer; HTTPS required) and optional
`reasoning_effort`, and always uses official OpenAI Responses.
`openrouter` takes required keyword-only `api_key` plus optional
`referer`/`title` attribution and targets
`https://openrouter.ai/api/v1/responses`. `ollama` stays keyless and
uses native `/api/chat`. `gateway` takes required `wire_protocol` and
`credential_name`. `e2b_sandbox` is a T4 leaf, not Landlock and not
isolated. Linked constructors do not attach a `MediaResolver`; vision,
file, and audio input require a host-built provider (ADR-049 rejected
FFI resolvers). OpenAI and OpenRouter output is capped at 128,000
tokens and Anthropic output at 64,000 tokens, the current Claude
ceiling; the linked context window is 1,050,000 tokens. Pass keys
explicitly; the binding does not read environment variables. Lazy
`finstack_ai.providers.*` stay unloaded until attribute access.
`Agent.re_resolve()` returns a new lock from reconstructed catalogs;
in-flight runs keep the previous composition. Optional
`approval_grant=` on every factory selects
`ApprovalGrantMode.per_call()` (default) or
`ApprovalGrantMode.informed_batch()`. `Policy` remains a mandatory
approval floor on every catalog.

Never put secrets in `AgentSpec`. See [provider security](provider-security.md).

`Agent.start` / `run` take optional `capability=` to select a model-activated
variant. `None` runs this agent. **Python `Capability` stays
instruction-only** (`id`, `description`, `instructions`, `activation`).
It does not accept toolset, context-provider, or middleware references.
See [capabilities](capabilities.md) and [FAQ](faq.md).
`Agent.from_python` accepts keyword-only `sqlite_path` and
`sqlite_durability`. `Lane.resume` continues a parked run after
`open_session`. `Run.start_child` prepares and accepts a child through
the Rust `ChildRunPrepared` / `AgentInvoker` handshake.
`Run.complete_external` routes an authenticated completion through the
same ingress as `WorkflowSession::complete_external`.
`normalize_prebeta_shape` remains a validator in front of that router.

## Learning notebooks

The [notebook series](../../examples/python-notebooks/) teaches
`Agent` as the composition root. There is no Python `Harness` type.
Notebooks 01–04, 08, and 10 stay offline. 05 constructs a local Ollama agent and runs
live when the server is reachable. 06–07 construct linked providers
offline and run live only when `OPENAI_API_KEY` or `ANTHROPIC_API_KEY`
is set. Notebook 06 takes `OPENAI_MODEL` from its first code cell
(`gpt-4o-mini` by default). Pass `api_key=` explicitly; the binding does
not read environment variables.

```bash
uv sync
uv run python -m ipykernel install --user --name=finstack-ai-notebooks \
  --display-name="finstack-ai-notebooks"
uv run python scripts/docs/notebooks.py
```

## License

[MIT](../../licenses/LICENSE-MIT) OR [Apache-2.0](../../licenses/LICENSE-APACHE).
[Contributing](../../CONTRIBUTING.md). [Security](../../SECURITY.md).
