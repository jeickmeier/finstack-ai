# finstack-ai-tools-openai-media

Native OpenAI image, speech, and transcription tools for finstack-ai. The
host supplies the API key and endpoint explicitly; the crate never reads
credentials from the environment.

Requests are bounded, cancellation- and deadline-aware, and redirect-free.
Caller-supplied transcription URLs are validated, resolved, address-pinned,
and blocked from private network destinations. Large generated media can be
staged through an optional `ArtifactStore` instead of being returned inline.

```rust
use finstack_ai_tools_openai_media::{OpenAiMediaConfig, OpenAiMediaToolset};

let tools = OpenAiMediaToolset::try_new(OpenAiMediaConfig {
    api_key: "explicit-secret".to_owned(),
    endpoint: String::new(),
    max_result_bytes: 256 * 1024,
})?;
# Ok::<(), finstack_ai_tools_openai_media::OpenAiMediaError>(())
```

Production endpoints must use HTTPS. Loopback HTTP exists only for local
fixtures. Keep API keys out of tool arguments, logs, and persisted state.
