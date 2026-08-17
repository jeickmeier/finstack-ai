# finstack-ai-context-repository

Trusted native `ContextProvider` that contributes a small allowlist of
repository instruction files (`AGENTS.md`, `README.md`, and
`.finstack/instructions.md` by default) from one authorized root.

File text is unprivileged data with file provenance (TM-01). The provider
does not execute tools discovered in those files and does not ingest the
whole repository. Construction fails closed on targets without capability-safe
directory primitives.

This crate is a T1 native adapter. It is not isolated.

```rust
use finstack_ai_context_repository::RepositoryContextProvider;

let provider = RepositoryContextProvider::try_new(".").expect("repository");
```
