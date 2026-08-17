# D1 — reqwest rustls spike (no provider switch)

Date: 2026-08-17
Baseline commit: `9781190f70e72bca1ea7e68351a346465104432a`
Method: isolated git worktree at `/tmp/finstack-d1-tls-spike`. Live workspace `reqwest` pin and provider `vendored-tls` features were not changed by this spike.
Tooling: `cargo-deny` 0.20.2 (`all-features = true` per `deny.toml`); `cargo tree --edges normal`.

## Recommendation

**D2 = NO-GO.** Both reqwest 0.13 rustls feature sets fail `cargo-deny` licenses on `CDLA-Permissive-2.0` (`webpki-root-certs` via `rustls-platform-verifier`). Do not switch providers. Do not broaden the license allowlist to force rustls (Slice D stop rule).

PR-024 rejected rustls because transitive TLS licenses were outside the then-narrow allowlist. `ISC` is now allowed and was **not** the failure this time. The new blocker is `CDLA-Permissive-2.0`, which is not in `deny.toml`.

## Feature sets tested

reqwest 0.13 renamed `rustls-tls` → `rustls`. Roots features (`rustls-native-certs` / `webpki-roots`) were removed; both rustls features enable `rustls-platform-verifier`.

| Variant | Workspace pin | Crypto | Deny |
| --- | --- | --- | --- |
| A (plan-named) | `["http2", "json", "rustls", "stream"]` | aws-lc-rs (reqwest default) **and** workspace `ring` (features are additive) | licenses FAILED — `CDLA-Permissive-2.0` |
| B | `["http2", "json", "rustls-no-provider", "stream"]` | workspace `rustls` + `ring` only | licenses FAILED — same `CDLA-Permissive-2.0` |

In the isolated tree, provider `vendored-tls` was remapped to `[]` so `all-features` deny would not still pull `native-tls-vendored`. reqwest 0.13 has no rustls equivalent of `native-tls-vendored`; ring already vendors its crypto.

If a later, separate license-policy decision allowlists `CDLA-Permissive-2.0`, prefer **variant B**. Variant A pulls `aws-lc-rs` / `aws-lc-sys` / `cmake` / `jni` / `quinn` into the lockfile and enables aws-lc on the shared workspace `rustls` crate (including `finstack-ai-server`).

## Deny

Command (worktree): `cargo-deny check` (same binary as `mise run supply-chain`; fuzz workspace not re-run).

Both variants:

```
error[rejected]: failed to satisfy license requirements
webpki-root-certs v1.0.9  license = "CDLA-Permissive-2.0"
  └── rustls-platform-verifier v0.7.0
      └── reqwest v0.13.4
          └── providers / finstack-ai-python

advisories ok, bans ok, licenses FAILED, sources ok
```

`ISC` (rustls / ring / webpki / untrusted) was already allowlisted and did not fail. `aws-lc-sys` licenses did not fail on variant A; the only license error was CDLA.

`webpki-root-certs` is the Linux/WASM fallback root bundle for `rustls-platform-verifier`. `deny.toml` scans those targets, so the crate is in-graph even on Darwin.

## Package-count delta

Unique `cargo tree --prefix none --format '{p}' --edges normal` lines (package + version):

| Package | native-tls baseline | A rustls+aws-lc | B rustls-no-provider |
| --- | --- | --- | --- |
| reqwest | 133 | 138 (+5) | 135 (+2) |
| finstack-ai-provider-openai-compatible | 195 | 198 (+3) | 195 (0) |
| finstack-ai-provider-anthropic | 195 | 198 (+3) | 195 (0) |
| finstack-ai-python | 218 | 221 (+3) | 218 (0) |
| finstack-ai-server | 154 | 154 (0) | 154 (0) |

Unique crate **names** on the openai-compatible provider (normal edges): 149 → 149 (swap, not shrink).

Provider name-set swap under variant B (leave → enter):

- Leave: `native-tls`, `hyper-tls`, `tokio-native-tls`, `tempfile`, `fastrand`, `errno`, `rustix`
- Enter: `rustls`, `hyper-rustls`, `tokio-rustls`, `rustls-platform-verifier`, `rustls-webpki`, `untrusted`, `subtle`

Lockfile package-name delta vs HEAD (variant B): **+11 / −9**.

- Added: `combine`, `jni`, `jni-macros`, `jni-sys`, `jni-sys-macros`, `rustls-native-certs`, `rustls-platform-verifier`, `rustls-platform-verifier-android`, `simd_cesu8`, `simdutf8`, `webpki-root-certs`
- Removed: `foreign-types`, `foreign-types-shared`, `hyper-tls`, `native-tls`, `openssl`, `openssl-macros`, `openssl-src`, `openssl-sys`, `tokio-native-tls`

`native-tls` is absent from the variant B lockfile (`cargo tree -i native-tls` fails). The `tempfile` ← `native-tls` edge is gone from the Python/provider graph. `security-framework` **stays** on Darwin via `rustls-platform-verifier`, so a D2 check that `security-framework` leaves `finstack-ai-python` would still fail.

Wheel / `check_size_budgets` was not run: deny already failed, and a wheel build would contend with other agents.

## D2 notes (not implemented)

- Do not change the workspace `reqwest` pin or provider `vendored-tls` features.
- A future switch, only after a license-policy decision, would use `rustls-no-provider` (not `rustls`) and `vendored-tls = []`.
- Providers remain native-only; rustls in the provider graph is not a WASM-forbidden-crate issue.

## Supporting files

- [d1-baseline-native-tls.txt](d1-baseline-native-tls.txt)
- [d1-variant-a-counts.txt](d1-variant-a-counts.txt)
- [d1-variant-a-deny-head.txt](d1-variant-a-deny-head.txt)
- [d1-variant-b-counts.txt](d1-variant-b-counts.txt)
- [d1-variant-b-deny-head.txt](d1-variant-b-deny-head.txt)
