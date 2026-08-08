# CI release-smoke fixture

Private, unpublished workspace binary used only by PR-003 release evidence.

- Package: `finstack-ai-ci-smoke`
- Depends on the public `finstack-ai` facade with native defaults
- Emits a deterministic identity line: `finstack-ai-ci-smoke <version>`
- Not a product CLI; do not add argument parsing or application APIs

Build locally with:

```bash
mise run release-smoke
```

On Linux, reproducibility is checked with:

```bash
mise run release-reproducible
```
