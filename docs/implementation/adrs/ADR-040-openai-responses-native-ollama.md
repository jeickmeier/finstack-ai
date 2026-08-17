# ADR-040: OpenAI Responses and native Ollama

## Status

Accepted

## Date

2026-08-17

## Accountable role

Ecosystem lead

## Decision owners

me@jeickmeier.com (finstack-ai maintainers)

## Context

ADR-023 made OpenAI-compatible Chat Completions the reference network
provider. Official OpenAI now requires the Responses API for current
reasoning models, tool loops, and structured output. Chat Completions
cannot preserve Responses `call_id` values, typed output items, or
encrypted reasoning across tool turns.

The shared `finstack-ai-provider-openai-compatible` crate also served
Ollama, Azure OpenAI, vLLM, LM Studio, and generic gateways. Those
endpoints do not implement `/v1/responses`. Keeping one Chat Completions
adapter as the official OpenAI path would leave newer models broken.

This record supersedes ADR-023. The scripted model remains the semantic
reference. Registry publication is not authorized by this decision.

## Decision

1. Official OpenAI uses a dedicated Responses provider
   (`finstack-ai-provider-openai`) and public factory `Agent.openai`.
   The wire path is `POST https://api.openai.com/v1/responses`.
2. Requests are stateless: `store` is always `false`. The adapter does
   not send `previous_response_id`.
3. Tool and reasoning continuation replays complete prior output items,
   including encrypted reasoning content, from opaque
   `ModelResponse.continuation_state`. Providers decode that blob; the
   kernel does not interpret it.
4. Function results preserve the exact provider `call_id`.
5. Ollama uses a dedicated native `/api/chat` provider
   (`finstack-ai-provider-ollama`). `Agent.ollama` remains the public
   factory. Native Ollama has no response or tool-call IDs;
   `provider_call_id` is omitted and completion identity may be derived
   from the committed `ModelRequestId`.
6. Generic OpenAI-compatible Chat Completions support is removed:
   `finstack-ai-provider-openai-compatible`, `Agent.openai_compatible`,
   `createOpenAICompatibleModel`, Azure OpenAI, vLLM, LM Studio, and
   gateway quirks have no replacement.
7. `ScriptedModel` remains the semantic reference. Provider quirks stay
   in adapters.
8. These removals are a SemVer-major public break. Workspace version
   fields stay `1.0.0` until a separately named publication decision.
   No deprecated aliases or compatibility crates are added.

## Consequences

- Official OpenAI tool loops can preserve reasoning items and `call_id`.
- Ollama no longer depends on a Chat Completions compatibility layer.
- Adopters of Azure, vLLM, LM Studio, or generic gateways must supply
  their own `Model` implementation.
- Browser/WASM Chat Completions adapter is replaced by a same-origin
  Responses adapter or a host model.
- Continuation JSON may contain encrypted reasoning and must not be
  logged, placed in telemetry, or copied into model-visible text.
- Durable replay of continuation state requires the runtime to pass the
  last successful `continuation_state` on the next `ModelRequest`.

## Rejected alternatives

**Keep Chat Completions as the official OpenAI path.** Rejected: current
OpenAI reasoning models reject or degrade Chat Completions tool use.

**Preserve `Agent.openai_compatible` as a local/gateway alias.**
Rejected: the user-selected scope removes Chat Completions entirely.

**Use `previous_response_id` with stored Responses.** Rejected for the
initial Responses provider: stored conversations change privacy and
reconciliation semantics. Stateless `store:false` replay is the first
contract.

**Make a proprietary provider SDK the semantic reference.** Rejected:
`ScriptedModel` remains the deterministic conformance reference.

## Compatibility and schema-change classification

Public provider-adapter and binding-factory removal. Scripted model
remains the semantic reference. Optional `provider_call_id` on tool-call
DTOs is additive when absent. Continuation state stays opaque `RawJson`
already present on `ModelResponse`. Journal record kinds are unchanged.
Workspace crate/wheel/npm version fields are not published by this ADR.

## Security classification

- References: SEC-INV-005; TM-04
- Threat Model review trigger: [§18](../../planning/06-finstack-ai-security-threat-model.md#18-review-triggers)
- Control-change disposition: Continuation blobs and encrypted reasoning
  are secret-bearing provider state. They must not appear in logs,
  observer payloads, or generated fixtures. Official OpenAI still
  requires HTTPS for credentials. Ollama remains keyless loopback.
  No new kernel port or trust boundary is added.

## Affected requirements, design, and delivery

- Affected requirements: FR-MDL
- Affected Technical Design: Technical Design §14 (reference provider)
  and workspace layout §2
- Architecture Specification §6.1 and §25
- PRD §18 decision 9
- Implementation Plan Phase 11 / PR-068–PR-073
- Architecture Specification §25 decision summary: ADR-040 supersedes
  ADR-023

### PRD §18 delivery detail

- Product decision: Official OpenAI uses Responses; Ollama uses native
  `/api/chat`; Chat Completions and generic compatible endpoints are
  removed; the scripted model remains the semantic reference.
- Delivery point: PR-068–PR-073. Publication remains a later named
  action.
- Source: [PRD §18](../../planning/01-finstack-ai-product-requirements.md#18-resolved-foundational-product-decisions)

## Supersession metadata

This ADR supersedes ADR-023. It is not superseded.

## Reconsideration conditions

May change only through a new superseding ADR and reconciliation of
every affected primary planning document. Reopening generic Chat
Completions, stored Responses (`previous_response_id`), or a third
network reference provider requires that path.

## Approval and implementation-evidence links

- Approval: accepted by the decision owner to execute the removal-first
  Responses and native Ollama migration without registry publication
- Implementation evidence: Partial until PR-068–PR-073 complete
