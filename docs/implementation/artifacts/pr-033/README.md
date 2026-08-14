# PR-033 artifacts

Owner: `me@jeickmeier.com`

| File | Purpose |
| --- | --- |
| [`plan.md`](plan.md) | Admission and execution envelope |
| [`bundle-size.json`](bundle-size.json) | Reporting-only wasm/glue sizes (no budget fail) |
| [`candidate-validation.txt`](candidate-validation.txt) | Focused and aggregate local validation at `dd674e7dff7cdb126d184a1cbd33dd5cb7df8053` |
| [`security-review.txt`](security-review.txt) | Threat Model section 18 npm-identity review |
| [`integration-validation.txt`](integration-validation.txt) | Local `main` merge `1781b8d4841d573a3bc2d82b7fe5a6abce5df145` |

This PR does not publish `@finstack/ai`, open a hosted pull request, flip
`DeferredBindingAdapter::wasm()`, or decide G4.
