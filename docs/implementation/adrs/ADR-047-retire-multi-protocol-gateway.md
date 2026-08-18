# ADR-047: Retire the multi-protocol gateway

## Status

Accepted

## Date

2026-08-18

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

ADR-040 required official OpenAI to use Responses, Ollama to use native
`/api/chat`, and Chat Completions plus generic compatible endpoints to be
removed. Phase 11 exit claimed that removal. `openai_chat` survived inside
`finstack-ai-provider-gateway` and `provider_util/openai_chat.rs`.

ADR-045 lists `Agent::gateway` as a first-class constructor. ADR-040
decision 6 (“gateway quirks have no replacement”) applies to generic Chat
Completions endpoints, not to deleting the constructor. The SDK already
depends on the three dedicated provider crates.

## Decision

Delete the multi-protocol gateway. Do not centralize its `json!` encoders.

- Delete `finstack-ai-provider-gateway`.
- Delete `WireProtocol::OpenaiChat`, `OpenAiChatAssembly`, and
  `provider_util/openai_chat.rs`.
- Keep `Agent::gateway` as a thin dispatcher onto the three dedicated
  providers. `openai_chat` becomes a configuration error. The constructor
  is not removed.
- Dedicated `*Config::try_new` already takes an arbitrary endpoint.
  Retarget must pass `spec.endpoint` into the dedicated config. Do not
  hardcode `https://api.openai.com`.
- `Agent::openai` / `::anthropic` / `::ollama` stay literal-key
  constructors. `GatewayAgentSpec` public fields stay unchanged.

This amends ADR-040: Phase 11 did not finish Chat Completions removal
while the protocol remained inside the gateway.

## Consequences

- Callers that pass `wire_protocol="openai_chat"` fail closed.
- Bindings keep the `Agent.gateway` signature.
- Findings that live only in the gateway crate die with it. Survivors in
  the three dedicated providers (`.expect`, unknown-model first-profile,
  `Unsupported` vs plumbed `structured`) are a later wave.

## Rejected alternatives

**Centralize the gateway `json!` encoders.** Rejected: the dedicated
crates already own their assemblies. Copying encoders into a new shared
home is not the deletion.

**Remove `Agent::gateway`.** Rejected: ADR-045 keeps the constructor.
Retirement is the crate and the Chat Completions path, not the SDK name.

**Keep `openai_chat` as a compatibility alias onto Responses.** Rejected:
ADR-040 removed Chat Completions. An alias would preserve the protocol.

## Compatibility and schema-change classification

Pre-1.0 source-breaking for `extensions/` and for the public
`OpenAiChatAssembly` / `openai_chat` names. `Agent::gateway` remains.
No journal `RecordBody`, WIT world, or remote-protocol meaning change.

## Security classification

- References: SEC-INV-005; TM-04
- Threat Model review trigger: none for this authorization record. The
  deletion wave reviews the retarget and the removed Chat Completions
  path. This record does **not** invent a review id.
- Residual: `Agent::gateway` still accepts a public string spec.

## Affected requirements, design, and delivery

- Affected Technical Design: provider batteries, SDK constructors.
  Planning files are amended in the same review unit that accepts this
  ADR (Phase 14).
- Implementation: Phase 14 deletion wave (PR-094). Authorization
  accepted this record; the crate deletion is local uncommitted work
  and has no published evidence id.
- Unchanged: `Agent::gateway` constructor; dedicated OpenAI / Anthropic /
  Ollama factories; no seventh port.

## Supersession metadata

Amends ADR-040. Does not supersede ADR-040 or ADR-045.

## Reconsideration conditions

May change only through a new superseding ADR. Removing `Agent::gateway`,
reintroducing Chat Completions, or adding a fourth in-tree protocol
requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to authorize Phase 14
- Implementation evidence: Missing (authorization only; no invented
  evidence or review id)
