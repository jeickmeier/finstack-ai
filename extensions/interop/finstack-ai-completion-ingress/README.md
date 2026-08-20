# finstack-ai-completion-ingress

Host-embedded ingress that mints signed opaque callback tokens for deferred
effects and delivers authenticated external completions to the runtime's
`ExternalCompletionRouter`. This is the wakeup path for deferred model,
tool, and sandbox jobs, and it is the workflow webhook.

This crate never reads environment variables and never discovers peers.
Callers are T4 remote principals; every token, signature, or body failure
is audited and answered with one non-existence-revealing rejection. The
configured idempotency horizon must be at least as long as callback-token
validity. This crate is a T4 native adapter. It is not compiled into
`wasm-host`.
