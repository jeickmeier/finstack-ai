# apps/

End-user products composed from released finstack-ai components.

What belongs here: applications (libraries with binaries) that assemble the
SDK, extensions, and bindings into something a person uses directly — a CLI,
a notebook track's shared definition, a product agent. What does not belong
here: port implementations, shared machinery, or anything another crate in
`crates/`, `extensions/`, or `bindings/` depends on.

Trust class: trusted native code, the same class as `extensions/`. Apps may
not implement runtime ports except by composing existing extensions, and
they never appear in `crates/` dependency graphs. Every crate here is
`publish = false`.

Current apps:

- `finstack-knowledge/` — the knowledge agent (`finstack-ai-knowledge`
  library + `finstack-know` CLI): ingest documents, remember facts across
  sessions, answer with retrieval and citations. See its README for the
  cross-surface parity matrix.
