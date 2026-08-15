# Examples

Public-API examples for finstack-ai bindings.

- [`browser-minimal/`](browser-minimal/) — experimental same-origin IndexedDB
  inspect demo. Persistence is not crash-durable and is labeled experimental
  until PR-048. The Dedicated Worker topology is the production default.
- [`ts-alpha-install/`](ts-alpha-install/) — clean TypeScript consumer that
  typechecks against a staged `@finstack/ai` tarball. No repository path
  mapping and no embedded provider credentials.
- [`durable-interaction/`](durable-interaction/) — typed interaction that
  survives a simulated worker restart on a SQLite journal via the local
  workflow driver. Not a default dependency of `finstack-ai-native-examples`.
