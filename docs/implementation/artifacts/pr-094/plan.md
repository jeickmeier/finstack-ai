# PR-094 execution plan

Date: 2026-08-18
Owner: me@jeickmeier.com
Plan baseline: documentation pack v0.27 / PLAN-0.25

This file is the execution contract for PR-094 (draft W8 / PR-089).

## Purpose

Retarget `Agent::gateway` onto the three dedicated providers, land
shared credentials on those crates, and delete the gateway crate plus
`OpenAiChatAssembly`.

## Principal changes

- Dedicated crates re-export shared `SecretString` / `Authentication` /
  `CredentialStore`. `with_authentication` inserts one named entry.
  `with_credential_store` binds a host store.
- `gateway_inner` dispatches to openai / anthropic / ollama.
  `openai_chat` is a configuration error. `spec.endpoint` and
  `spec.hard_input_bytes` are passed through. One-entry store from
  `credential_name`.
- Delete `finstack-ai-provider-gateway`, `OpenAiChatAssembly`, and
  `provider_util/openai_chat.rs`.
- Port `check_model_conformance` onto openai / anthropic / ollama.

## Acceptance mapping

- PR-094-A01: `Agent.gateway(..., wire_protocol="openai_chat")` fails closed.
- PR-094-A02: each crate's `try_new` still returns its own stable code.
- PR-094-A03: gateway construct for `openai_responses` still succeeds.

## Explicit exclusions

W9 `.expect` / profile / structured-output fixes. Temporal deletion.

## Dependencies

PR-085.
