# finstack-ai-provider-openai

Official OpenAI Responses (`POST /v1/responses`) implementation of
`finstack_ai_runtime::Model`.

Requests are stateless (`store: false`) and never send `previous_response_id`.
Tool-loop continuation replays complete prior output items from opaque
`ModelResponse.continuation_state`. Credentials and secret headers require
HTTPS. Keyless HTTP is allowed only for local loopback tests.

This crate is a T1 native adapter. It is not isolated.
