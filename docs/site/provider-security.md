# Provider and tool security

- Use an explicit endpoint family and HTTPS for credentialed providers.
- Supply bearer/API-key values only through the provider's redacted secret
  wrappers. Configuration URLs reject embedded credentials, query strings, and
  fragments.
- Keep `AgentSpec`, bundle defaults, resolution locks, metadata, examples, and
  diagnostics credential-free. Locks store digests and exact identities only.
- Treat every in-process provider and Toolset as trusted native code. Review
  endpoint behavior, dependency changes, tool schemas, side-effect classes,
  retry safety, approval requirements, resource ceilings, and cancellation.
- Calculator execution is read-only and bounded. Filesystem execution is
  capability-scoped to an explicit root; write/edit tools are approval-gated by
  the native facade.
- Do not authorize a broad filesystem root such as a home directory or `/`.
  Prefer a dedicated project subtree with protected secret/config patterns.
