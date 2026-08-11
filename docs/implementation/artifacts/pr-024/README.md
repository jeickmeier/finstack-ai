# PR-024 local delivery evidence

PR-024 implements the trusted native OpenAI-compatible Chat Completions
provider for the public runtime `Model` port. The immutable implementation
candidate is commit `54cb0e40612161abddb62eda1ffb03b99819e267` with tree
`3c3d79a42e0405808ce37f31dbee4b482f421f36` on
`codex/pr-024-openai-provider`.

The exact candidate command inventory is in
[`candidate-validation.txt`](candidate-validation.txt), and the security and
threat-model disposition is in [`security-review.txt`](security-review.txt).
Local integration evidence will be added after the candidate evidence commit is
merged to `main` and post-merge gates pass.

## Local acceptance map

| Acceptance | Immutable candidate proof | Disposition |
| --- | --- | --- |
| A01 | Recorded loopback fixtures pass for streamed text, fragmented tool calls, native structured JSON, cumulative usage, retryable HTTP errors, and cancellation | Passed on candidate |
| A02 | Minimal kernel/runtime checks, architecture checks, and normal dependency trees prove the provider and reqwest do not enter kernel/runtime production graphs | Passed on candidate |
| A03 | Canary tests, HTTPS-only credentials, keyless-only HTTP, secret-safe Debug/errors, bounded SSE, and aggregate secret scanning pass | Passed on candidate |
| A04 | The optimized warm pooled-client loopback round-trip benchmark compiles in the focused gate and executes successfully on the candidate | Passed on candidate |
| A05 | One provider package passes keyless local compatibility fixtures, while the existing scripted-model semantic-reference suite passes unchanged | Passed on candidate |

The implementation adds no provider router, OAuth flow, Responses API, browser
adapter, credential acquisition, kernel port, or semantic-model replacement.
The optional live smoke remains ignored and was not run. No hosted run, actual
pull request, push, publication, or independent review is claimed.

