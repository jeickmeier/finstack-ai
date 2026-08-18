# Capabilities

Declarative capabilities are a finite catalog on an agent. They contribute
instructions, and on native Rust they may also contribute registered
Toolset, ContextProvider, and Middleware handles.

## Run-start selection

`AgentRunRequest.capability` (Rust) or `capability=` on `Agent.start` /
`Agent.run` (Python / WASM) selects one **model-activated variant** before
the run starts. `None` runs the agent that was called. An unknown catalog
id fails closed. Word overlap in user input does not select a capability.

`capability_catalog()` / `compact_capability_catalog()` stay public.

## Mid-run activation

Native hosts may attach `finstack-ai-tools-skills`. `capability_list`
renders the compact catalog. `capability_activate` is **additions-only**:
the runtime computes `current ∪ named` and journals that complete set.
Naming one id does not drop already-active capabilities.

Activation is gated at dispatch with an active mask over the lock-time
union. Mid-run activation does not re-resolve the agent.
`Agent.re_resolve()` / `Agent.reResolve()` is a new composition and a
new lock (ADR-046). `middleware_chain_digest` stays constant for the
run (ADR-041 option b). A second `ContextCompactor` in an inactive
capability fails **resolution**.

Recovery rebuilds the mask from `ResolvedAgentLock` plus the journaled
`active_capabilities` chain, or fails closed.

## Binding parity

**Python stays instruction-only.** `Capability(id, description,
instructions, activation=…)` does not accept toolset, context-provider,
or middleware references. WASM matches that instruction-only surface.
First-class constructors and `re_resolve` exist on both bindings;
wasm-host fail-closed is a Rust platform error, not a missing method.

Untrusted / `model` capabilities cannot set
`trusted_application_instructions`. Capability instructions are never a
path to that flag.

## Prompt-cache invalidation

Mid-run activation changes the tool list on the next model request.
Anthropic and OpenAI/gateway adapters bust the stale prefix: the next
request omits `cache_control` on the old system block and does not send
a cache key that claims the previous tool list. The vendor prefix is
still invalid; this is an honest bust, not cache preservation.

Load-time `SKILL.md` import is composition-time and default-off
(`finstack-ai-tools-skill-import`, ADR-044). It is not a per-run scan.
