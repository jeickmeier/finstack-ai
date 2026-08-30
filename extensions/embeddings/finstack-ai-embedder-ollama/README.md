# finstack-ai-embedder-ollama

Native Ollama `POST {base_url}/api/embed` implementation of the finstack-ai
`TextEmbedder` contract from `finstack-ai-embeddings`.

The crate mirrors the Ollama *provider's* HTTP conventions — its own pooled
`reqwest` client, redirect following disabled, and `vendored-tls` feature
parity — rather than `finstack-ai-net-guard`, whose loopback/private-IP
vetting is the wrong default for an operator-configured localhost daemon.
The base URL is validated once at construction: HTTP is allowed only toward
a loopback IP, and credentials, query, and fragment components are rejected.

Configuring this embedder is an application's explicit egress decision;
no finstack-ai component ever selects an embedder by default. The embedder
identity is `embed.ollama.<model>.<dimensions>` — a changed model revision
is a new identity, that is, a new embedding space.
