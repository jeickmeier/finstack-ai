# finstack-ai-tools-openrouter-media

OpenRouter image, video, speech, and transcription tools for finstack-ai. The
host supplies credentials and routing metadata explicitly; the crate does not
read environment variables.

Requests and response bodies are bounded, cancellation- and deadline-aware,
and redirect-free. Transcription downloads use vetted, address-pinned URLs and
reject private network destinations. Generated media can be staged through an
optional `ArtifactStore`.

```rust
use finstack_ai_tools_openrouter_media::{
    OpenRouterMediaConfig, OpenRouterMediaToolset,
};

let tools = OpenRouterMediaToolset::try_new(OpenRouterMediaConfig {
    api_key: "explicit-secret".to_owned(),
    endpoint: String::new(),
    referer: None,
    title: None,
    max_result_bytes: 256 * 1024,
})?;
# Ok::<(), finstack_ai_tools_openrouter_media::OpenRouterMediaError>(())
```

Production endpoints must use HTTPS. Keep credentials out of model-visible
arguments, logs, and persisted state.
