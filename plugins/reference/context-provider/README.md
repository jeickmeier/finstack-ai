# Context-provider reference component

Isolated `context-plugin` guest with identity
`finstack.plugin.reference.context`. Item JSON matches the in-process
`ReferenceContextProvider` guest in `finstack-ai-wit`.

Rebuild with `mise run gen-plugin-wasm`. Host conformance lives in
`finstack-ai-plugin-host` (`context_provider_matches_in_process_reference`).
