# TM-22 proposed register row (change-control handoff)

Status: **drafted, not landed.** `docs/planning/` is read-only during normal
coding, so this row is staged here for the repository owner to apply to
`docs/planning/06-finstack-ai-security-threat-model.md` §7 (threat and
control register). No PR number is assigned to the bounded-HTTP-fetch
feature yet; the "delivery" column names the feature and should be renumbered
to `PR-NNN` when the owner assigns one.

Source: `docs/superpowers/specs/2026-08-20-bounded-http-fetch-design.md` §5.
The row below updates the spec's draft text to match what actually shipped
(commit `2870dc46e523bcbf67408a46409767d7000c255f`).

## Mapping and gate satisfaction

- **SEC-INV-006** (isolated extensions have no ambient network access):
  the fetch toolset is a *trusted native* battery, not an isolated one, so it
  does not weaken SEC-INV-006 — but it is the first native surface whose
  destination is chosen by model-supplied input, so its egress authority is
  bounded explicitly by a deny-by-default allowlist rather than by ambient
  process reachability. Both crates are opt-in dependencies; no default SDK
  feature pulls them in (SEC-INV-012 posture preserved).
- **SEC-INV-007** (all untrusted lengths/time/retained output bounded before
  resource commitment): response bodies are capped by
  `min(config.max_response_bytes, args.max_bytes)` and exceeding the cap is an
  error, never a truncation; redirect hops are capped; per-request timeout and
  the run cancellation/deadline gate bound time; inline result size is
  re-checked before the body reaches the model; oversize/binary bodies go to
  the scoped `ArtifactStore` instead of the transcript.
- **§15 deployment configuration gates** — "Network egress policy for native
  tools/providers and isolated capabilities: deployment owner before
  privileged network access." The fail-closed requirement is satisfied at the
  code level: `HttpFetchToolset::try_new` **refuses to construct** with an
  empty allowlist (`allowlist_empty`), and every numeric limit outside its
  hard ceiling is a construction error rather than a silent clamp. An
  unconfigured deployment therefore has no fetch tool at all.
- **§17 traceability** — the "Scoped resource authority" row
  (NFR-SEC-004; FR-TLS; FR-PLG) should gain this feature alongside PR-025 /
  PR-052 / PR-056, since the allowlist + destination deny is the same
  scoped-authority pattern applied to network egress.
- **§18 review triggers** — fired by "adds a ... privileged host capability"
  and "adds a protocol, parser ... or secret-bearing adapter" (HTML parsing
  via `htmd`/`html5ever`; per-host credential headers). Closure evidence:
  `security-review.txt` in this directory.
- **§16 residual-risk register** — no new row is required; the existing
  "Bounded parsers/queues reduce but do not eliminate remote
  resource-exhaustion" and "Models and retrieved content remain untrusted"
  rows already cover the residuals recorded in the security review.

## Proposed row

Columns: `ID | Threat | Required controls | Primary verification and delivery`

| TM-22 | SSRF / private-network egress via model-supplied URLs in the fetch toolset. | Deny-by-default host allowlist (empty allowlist fails construction; exact-host or explicit `*.suffix` patterns only); HTTPS-only on port 443, plaintext HTTP solely for loopback fixture hosts under an explicit `allow_loopback_http` flag; userinfo and fragment components rejected; literal and resolved loopback/private/link-local/unique-local/unspecified/broadcast/multicast address deny with IPv4-mapped-IPv6 canonicalization; DNS resolve-and-pin closing the rebinding TOCTOU, where one forbidden address rejects the whole answer set; client-level redirects disabled with manual per-hop re-vetting (allowlist, scheme, destination, fresh resolve and fresh pinned client) restricted to 301/302/303/307/308 and capped by `max_redirects`; the loopback allowlist bypass gated on a loopback *origin* hop so an allowlisted public host cannot redirect a chain into loopback; bounded body reads where exceeding the cap is an error, never a truncation; credential headers attached only on an exact per-host key match, so they never travel across a redirect to another host; oversize/binary bodies staged to the tenant/session/run-scoped artifact store. | Unit corpus and scripted-resolver rebinding tests in `finstack-ai-net-guard`; loopback fixture-server pipeline tests (allowlist denial, private-destination block, redirect chain, cross-host header isolation, cap and timeout paths) in `finstack-ai-tools-fetch`; security review artifact `docs/implementation/artifacts/bounded-http-fetch/security-review.txt`; delivery: bounded HTTP fetch feature (`finstack-ai-net-guard` + `finstack-ai-tools-fetch`). |

## Owner note

The security review recorded one **Important** finding (inherited
environment/system HTTP proxy support in the pinned `reqwest` client, which
would void the resolve-and-pin control where a proxy is configured). If that
is fixed before the row lands, no row text changes; if it is accepted as a
deployment constraint instead, the required-controls column should gain
"outbound proxy disabled" and §16 should gain a residual-risk row.
