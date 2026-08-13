# finstack-ai Python bindings

PR-027 establishes the `finstack_ai` PyO3 extension package, typed facade,
editable development, and per-version wheel pipeline. The current public
surface intentionally contains only side-effect-free health/build metadata and
linked-provider discovery; Agent and callback APIs arrive in PR-028–PR-031.

```bash
mise run python-develop
mise run test-python
```

The initial distribution links the Rust-backed OpenAI-compatible provider into
the same extension module and exposes its Python namespace lazily. Importing
`finstack_ai` does not create a provider client, initialize Tokio, read
credentials, or open network resources. Classic `abi3` wheels are not part of
the launch strategy.

See the repository [README](../../README.md) for project bootstrap and documentation routing. License texts are centralized under [`../../licenses/`](../../licenses/).
