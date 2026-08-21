# Toolset leaves

| Crate | Role |
| --- | --- |
| `finstack-ai-tools-calculator` | Bounded read-only arithmetic |
| `finstack-ai-tools-document` | Artifact-backed document parsing and PDF classification |
| `finstack-ai-tools-elicitation` | Durable human-in-the-loop questions |
| `finstack-ai-tools-filesystem` | Capability-scoped root; no symlink escape |
| `finstack-ai-tools-mcp` | Allowlisted MCP servers; no catalogue |
| `finstack-ai-tools-openai-media` | Explicit-route OpenAI image, speech, and transcription tools |
| `finstack-ai-tools-openrouter-media` | Explicit-route OpenRouter image, video, speech, and transcription tools |
| `finstack-ai-tools-shell` | Deny-by-default argv, empty env, timeout |
| `finstack-ai-tools-subagent` | Allow-listed child start over `AgentInvoker` |
| `finstack-ai-tools-skills` | `capability_list` / additions-only `capability_activate` |
| `finstack-ai-tools-skill-import` | Composition-time `SKILL.md` importer; catalog default-off |
| `finstack-ai-sandbox-e2b` | T4 remote sandbox; no compensating DELETE on cancel |
| `finstack-ai-tools-fetch` | Deny-by-default HTTPS allowlist; resolve-and-pin; bounded reads |
