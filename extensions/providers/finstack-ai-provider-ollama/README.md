# finstack-ai-provider-ollama

Native Ollama `POST {base_url}/api/chat` implementation of the finstack-ai
`Model` port.

The adapter streams NDJSON (not SSE). Keyless HTTP loopback is allowed; bearer
credentials require HTTPS. Ollama has no provider tool-call or response IDs —
`provider_call_id` is omitted and completion identity is the committed model
request id.

```rust
use finstack_ai_provider_ollama::{OllamaConfig, OllamaModelConfig, OllamaProvider};
use finstack_ai_runtime::Model;

let config = OllamaConfig::try_new("http://127.0.0.1:11434").expect("config");
let model = OllamaModelConfig::try_new("gemma3", 1_000_000, 128_000, 4_096, 4_096, 256)
    .expect("model");
let provider = OllamaProvider::try_new(config, vec![model]).expect("provider");
assert_eq!(provider.descriptor().provider.as_ref(), "ollama");
```
