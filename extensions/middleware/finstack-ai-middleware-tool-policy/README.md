# finstack-ai-middleware-tool-policy

Policy-driven filter middleware that narrows the model-visible tool set at
`before_model` and `before_tool_batch` stages. Outcomes intersect with any
other `FilterTools` leaf in the chain; the fold reports the intersection.

## What it does

Four independent rules can be combined:

1. **Role allowlist**: Restricts tools by granted role. Per-role allow sets are
   unioned with a default allow set; tools outside that union are dropped. No
   role rule → no narrowing.

2. **Write budget**: Per-run cap on non-`ReadOnly` tool calls (both
   `IdempotentWrite` and `NonIdempotentWrite`). Once the limit is reached,
   all non-`ReadOnly` tools are dropped from the retain set. This is a
   visibility brake on what the model can still request, not an audited
   ledger: it counts calls against the full tool list in the draft; only
   tools absent from the draft entirely are excluded.

3. **Jailbreak triggers**: Case-insensitive substring scan of `MessageRole::User`
   text and `MessageRole::Tool` tool result content (nested in `ToolResult`
   blocks). On match: either fail the stage outright, or intersect the retain
   set with a safe tool subset. Assistant and system text is trusted and
   never scanned. Pattern scan is bounded by the draft's own ceilings.

4. **Child-depth gate**: Once the run's child-agent nesting depth reaches a
   threshold, drop a configured set of tools. The middleware receives
   `current_depth` at construction time (not run time) because the middleware
   cannot inspect the dispatch depth at invocation; compose with the SDK's
   `ChildRunPolicy` to enforce depth limits across multiple layers.

## Deployment and stages

| Stage | Behavior |
| --- | --- |
| `before_model` | All four rules apply. Narrows the tool universe from `input.request.tools`. Returns `Continue` when nothing narrows, `FilterTools(retain)` when a rule narrows, or `Fail` when a jailbreak trigger with `JailbreakAction::Fail` matches. |
| `before_tool_batch` | Only role allowlist and child-depth gate apply (if configured); write budget and jailbreak scan do not run. Must return a complete allow set (derived from the role config alone, even if a depth gate subtracts from it). Returns `Continue` if no role allowlist is configured, otherwise `FilterTools(retain)`. Malformed payload is a hard `MiddlewareError`, not a stage outcome. Write-budget and jailbreak restrictions are **not** re-checked at this stage: a write-class tool call that a role allows passes this backstop even after the write budget was exhausted or a jailbreak pattern matched at `before_model`, since those two rules only ever narrowed the earlier `before_model` view and are not re-evaluated here. |

The `before_tool_batch` simplification exists because a filtered call becomes a
synthetic denial (not a silent drop): a leaf can only emit a retain set it knows
to be complete, so the role policy alone is sufficient — depth gates only remove
tools, preserving completeness.

## Configuration example

```rust
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

use finstack_ai_kernel::{ComponentId, ComponentRef, ToolId, Version};
use finstack_ai_middleware_tool_policy::{
    ToolPolicyConfig, ToolPolicyMiddleware, JailbreakAction,
};

fn tid(s: &str) -> ToolId {
    ToolId::parse(s).expect("tool id")
}

fn component_ref(id: &str, version: (u32, u32, u32)) -> ComponentRef {
    ComponentRef::new(
        ComponentId::parse(id).expect("component id"),
        Some(Version {
            major: version.0,
            minor: version.1,
            patch: version.2,
        }),
    )
}

// Build configuration with all four rules.
let config = ToolPolicyConfig::try_new()
    .expect("empty config")
    .with_role_allowlist(
        BTreeMap::from([
            (
                Arc::from("reader"),
                BTreeSet::from([tid("finstack.tools.read")]),
            ),
            (
                Arc::from("editor"),
                BTreeSet::from([
                    tid("finstack.tools.read"),
                    tid("finstack.tools.write"),
                ]),
            ),
        ]),
        BTreeSet::from([tid("finstack.tools.info")]), // default for unmapped roles
    )
    .expect("roles")
    .with_write_budget(3)
    .expect("budget")
    .with_jailbreak_triggers(
        vec![
            Arc::from("ignore previous instructions"),
            Arc::from("system override"),
        ],
        JailbreakAction::Fail,
    )
    .expect("jailbreak")
    .with_child_depth_gate(
        0,  // current_depth at construction
        2,  // max_depth: restrict when depth >= 2
        BTreeSet::from([tid("finstack.tools.spawn-agent")]),
    )
    .expect("gate");

// Construct the middleware leaf.
let middleware = Arc::new(ToolPolicyMiddleware::try_new(config).expect("leaf"));

// Register via NativeAgentBuilder.
let component = component_ref("finstack.middleware.tool-policy", (1, 0, 0));
builder.middleware(component, middleware);
```

## Stable error codes

### Construction-time errors

Build-time configuration errors are returned as `ToolPolicyError::Configuration`
with a stable `reason: &'static str`. These errors occur in the builder chain
(`ToolPolicyConfig::try_new` and `with_*` methods). Possible reasons:

- `duplicate_rule` — a rule slot (role, write, jailbreak, depth) already set
- `too_many_roles` — exceeds 128 roles
- `too_many_tools` — exceeds 1,024 tools per set
- `too_many_patterns` — exceeds 64 jailbreak patterns
- `empty_role_name`, `role_name_too_long`, `role_name_contains_nul` — role validation
- `empty_pattern`, `pattern_too_long`, `pattern_contains_nul` — pattern validation
- `depth_exceeds_kernel_cap` — depth exceeds kernel run-relation cap (16)
- `empty_policy` — zero rules configured (no-op policy rejected)
- `invalid_configuration_encoding`, `invalid_component_id` — serialization or identity errors

### Runtime stage-failure codes

When a jailbreak trigger with `JailbreakAction::Fail` matches during `before_model`,
the stage fails with `ErrorDescriptor.code = "tool_policy_jailbreak_triggered"`.
The message displays `"tool policy jailbreak trigger matched"`.

After construction, all policy evaluation is pure and deterministic; no other runtime
failures are possible.

## Source of truth

The complete behavior contract lives with the code:

- `src/config.rs` — validated configuration types and builder API.
- `src/eval.rs` — pure policy evaluation for both stages, including rule
  semantics and the complete-set caveat for `before_tool_batch`.
- `src/lib.rs` — middleware invocation, stage masking, and outcome mapping.

This crate is a T1 native adapter. It is not isolated.
