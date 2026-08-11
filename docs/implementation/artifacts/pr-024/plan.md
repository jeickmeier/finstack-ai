# PR-024 execution plan

Date: 2026-08-11

## Execution envelope

- mode: integrated
- branch: `codex/pr-024-openai-provider`
- baseline: `763607599b447460642bd5b101f98f785491ad9d`
- integration target: `main`
- authorized local actions: branch, edit, test, commit, and merge
- authorized external actions: none

## Contract and scope

PR-024 implements the trusted native OpenAI-compatible Chat Completions
reference provider under `extensions/providers/`. It implements the existing
public `Model` port without changing kernel semantics or making wire DTOs public
runtime contracts. The scripted model remains the semantic reference.

The implementation sequence is:

1. Add strict secret-safe provider configuration, a reused HTTP client, bounded
   model profiles, and a versioned endpoint-quirks table.
2. Map canonical messages, tools, structured output, and allowlisted settings to
   private Chat Completions wire DTOs.
3. Implement bounded SSE parsing and normalized text/tool/usage/completion
   events with HTTP error, timeout, and cancellation classification.
4. Add recorded fixtures and a keyless local HTTP compatibility harness for
   fragmentation, tool calls, structured output, retryable errors, and cancel.
5. Add measured request-translation and reused-client warm-path benchmarks plus
   an opt-in ignored live smoke test.
6. Run focused, dependency, architecture, security, benchmark, and aggregate
   validation; bind immutable candidate and local integration evidence.

## Acceptance map

- A01: recorded/local streaming text, tool calls, structured output, retryable
  errors, and cancellation pass.
- A02: kernel-only and runtime-only normal dependency graphs omit the provider
  and its HTTP stack.
- A03: canary credentials never appear in Debug, errors, request snapshots,
  traces, or recorded fixtures.
- A04: request-translation and reused-client warm-path overhead are benchmarked.
- A05: one provider package passes keyless local compatibility fixtures while
  the scripted model remains the semantic reference.

## Security and decision review

- ADR-023 is implemented without changing its Chat Completions baseline or the
  scripted semantic reference.
- TM-04/SEC-INV-005 review covers credentials, configured headers, URL handling,
  response bodies, provider errors, request IDs, recorded fixtures, and Debug.
- Redirects are disabled; response and SSE bytes are bounded before retention;
  unknown/malformed frames fail closed; optional live tests remain ignored and
  require explicit environment configuration.
- No new ADR trigger is identified at admission.

## External documentation verification

Context7 official OpenAI documentation confirms Chat Completions streaming
chunks, structured `response_format`, tool-call deltas, request identifiers,
and final `include_usage` behavior. Context7 official reqwest documentation
confirms one reused Client owns connection pooling, redirects can be disabled,
and response bodies can be consumed incrementally.

## Exclusions

No provider router, OAuth flow, full model catalog, Responses API implementation,
browser adapter, credential acquisition, live network call during ordinary
tests, publication, hosted pull request, or push is included.
