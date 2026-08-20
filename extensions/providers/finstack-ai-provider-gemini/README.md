# finstack-ai-provider-gemini

Native Gemini `generateContent` provider for the public `finstack-ai-runtime`
`Model` port. It talks `POST {base}/{model-path}:streamGenerateContent?alt=sse`
directly (always-streaming, unnamed SSE `data:` frames), preserving native
Gemini surfaces that a text-only OpenRouter route would drop: thought
signatures, Google Search grounding, code execution, context caching, and
native audio/video/file input via `inlineData`/`fileData`.

`GeminiEndpoint` is an auth/URL variant, not a separate protocol: the
`GenerativeLanguage` variant targets the public Generative Language API with
an `x-goog-api-key` credential, while the `Vertex { project, location }`
variant targets Vertex AI's `aiplatform.googleapis.com` host with a bearer
OAuth token supplied by the host's `CredentialStore`. Both variants send an
identical wire body; only URL construction and the credential header shape
differ. This crate is a T1 native adapter and a leaf: neither the kernel nor
the runtime depends on it, and it is not isolated.
