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

```rust
use finstack_ai_completion_ingress::{CompletionIngress, CompletionIngressConfig};

let ingress = CompletionIngress::try_new(store, audit_gate, CompletionIngressConfig {
    key_id: "k-2026-08".to_owned(),
    key: signing_key,                       // SecretString, >= 32 bytes
    additional_verification_keys: vec![],   // retired keys during rotation
})?;
let token = ingress.mint(&grant)?;          // hand token.as_str() to the external job
// later, from the host's transport terminator or the workflow webhook:
let outcome = ingress.deliver(received_token, received_body, now).await?;
```

The signing key should be at least 32 bytes of cryptographically random
material (e.g. base64 or hex of 32 random bytes), not a human-chosen
passphrase — `MIN_KEY_BYTES` checks length, not entropy.

Minted tokens are signed but not encrypted (locator ids are non-secret).
Duplicate deliveries with an equal body are idempotent; conflicting
duplicates fail closed with durable rejection evidence. Interaction
resolutions are out of scope for this crate.

Callers must enforce their own transport-level body size cap before
calling `deliver`: the rejection path digests the full received body, so
`MAX_BODY_BYTES` bounds acceptance, not the work done to reject oversize
garbage.
