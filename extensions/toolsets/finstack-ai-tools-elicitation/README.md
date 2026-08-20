# finstack-ai-tools-elicitation

Human-in-the-loop elicitation `Toolset` for finstack-ai.

Exposes tools that ask the user a question and park the run durably until a
resolution arrives:

- **`ask_user`** (free-form) — the model supplies the prompt and picks a
  profile: `free_text` (default), `choice` (with `options`), or `form` (with
  an inline JSON `response_schema`).
- **Typed per-workflow tools** — registered at build time with a fixed
  prompt, interaction kind, and response schema. The model supplies only
  call-time `context`; it cannot reshape the question contract.

Every call returns the reserved `tool_interaction_required` error with the
serialized `InteractionRequest` in the metadata. The runtime intercepts it,
journals the pending interaction, and moves the run to `AwaitingInteraction`.
The user's `InteractionResolution` response becomes the tool result the model
sees on resume. Answer via `RunHandle::resolve_interaction`, the server's
`interaction_resolved` command, or the Python/WASM bindings.
