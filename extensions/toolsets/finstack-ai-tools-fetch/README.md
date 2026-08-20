# finstack-ai-tools-fetch

Bounded, allowlisted HTTP `GET` fetch toolset for finstack-ai. Deny-by-default:
`HttpFetchToolset::try_new` refuses to construct with an empty host allowlist,
and every numeric limit is a construction-time error rather than a silent
clamp when it exceeds the crate's hard ceiling.

This task-5 slice provides the configuration layer only — `HostPattern`
parsing/matching and `HttpFetchConfig` validation. The `Toolset` port
implementation (request execution, redirect handling, response streaming)
lands separately.

```rust
use finstack_ai_tools_fetch::{HttpFetchConfig, HttpFetchToolset};

let config = HttpFetchConfig {
    allowlist: vec!["docs.rs".to_owned()],
    ..HttpFetchConfig::default()
};
let toolset = HttpFetchToolset::try_new(config).expect("fetch config");
```
