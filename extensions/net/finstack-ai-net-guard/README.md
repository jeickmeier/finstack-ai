# finstack-ai-net-guard

Vetted outbound HTTP primitives for finstack-ai: URL vetting, private-address
deny, DNS resolve-and-pin, and bounded body reads. Shared foundation for any
toolset that needs to fetch untrusted, caller-supplied URLs safely.

## Redirect loops

`client::pinned_client` disables reqwest's own redirect following on
purpose: the caller owns the hop loop, and with it the obligation to
re-vet. Every hop — not just the first — must run the full sequence again
before the next request is sent: `parse_and_vet_url` → allowlist check →
`resolve_and_pin` → a fresh `pinned_client` for that hop's pinned address.
Reusing a client, or skipping vetting on hop N because hop 0 already
passed, reopens the DNS-rebinding/SSRF gap this crate exists to close.

Any loopback or fixture-only bypass must be decided once from the
*original* (hop-0) URL and carried forward unchanged, never recomputed per
hop — otherwise an allowlisted public host can redirect into loopback and
inherit a bypass that was only ever meant for the origin. See
`finstack-ai-tools-fetch`'s `host_allowed`/`origin_is_loopback`
(`pipeline.rs`) for the reference implementation.
