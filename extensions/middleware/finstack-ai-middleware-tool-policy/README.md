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
   threshold, drop a configured set of tools. The gate compares the run's
   **live** relation depth (`RunCallContext::relation_depth`, sourced from the
   accepted run's own `RunRelation.depth`) against `max_depth` at every stage
   invocation — there is no construction-time depth to go stale, and no need
   to build a separately-configured agent per depth. This composes with (does
   not replace) the SDK's `ChildRunPolicy`, which enforces depth limits at
   dispatch time. `max_depth = 0` is degenerate: `relation_depth >= 0` always
   holds, so the gate fires for every run, including the root run.

## Deployment and stages

| Stage | Behavior |
| --- | --- |
| `before_model` | All four rules apply. Narrows the tool universe from `input.request.tools`. Returns `Continue` when nothing narrows, `FilterTools(retain)` when a rule narrows, or `Fail` when a jailbreak trigger with `JailbreakAction::Fail` matches. |
| `before_tool_batch` | Role allowlist and child-depth gate apply, narrowed against the **real carried universe** (`input.tools`, the resolved catalog for this run) via the same `narrow_universe` rules `before_model` uses — a depth-only or role-only config now narrows here just as it would at `before_model`. Write budget and jailbreak scan do not run at this stage: both need message history (prior tool-call blocks, user/tool text) that `BeforeToolBatchInput` deliberately does not carry, and the narrowed `before_model` request is not retained in kernel state for this stage to re-read. Returns `Continue` when the narrowed retain set equals the carried universe, otherwise `FilterTools(retain)`. |

This stage exists as a backstop against a provider emitting calls to tools the
model was never shown: `before_model` already hid ineligible tools from the
model at the source, and `before_tool_batch` re-checks role and depth rules
against the true universe a provider could otherwise route around by ignoring
the narrowed request. `StageOutcome::FilterTools` is retain-semantics — the
runtime turns whatever is *not* retained into a per-call `Deny` →
`SyntheticClosure`, never a silent drop — so this only works because the stage
now has a concrete universe (`input.tools`) to narrow, the same shape
`before_model` narrows from `input.request.tools`.

## Configuration example

Construct via `ToolPolicyConfig::new().with_*()`. Do not inspect rule structs:
`RoleAllowlist`, `WriteBudget`, `JailbreakTriggers`, and `ChildDepthGate` (and
the `ToolPolicyConfig` inspection accessors) are crate-private. `Default` is
not implemented.

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
let config = ToolPolicyConfig::new()
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
        2,  // max_depth: restrict when live relation_depth >= 2
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
(the `with_*` methods; `ToolPolicyConfig::new` is infallible). Possible reasons:

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

The runtime guarantees a typed `BeforeToolBatchInput` at `before_tool_batch`
(there is no malformed-payload case to guard against); invoking this leaf at
a stage other than `before_model`/`before_tool_batch` is the only remaining
runtime failure, surfaced as `MIDDLEWARE_OUTCOME_NOT_ALLOWED`.

After construction, all policy evaluation is pure and deterministic; no other runtime
failures are possible.

## Source of truth

The complete behavior contract lives with the code:

- `src/config.rs` — validated configuration types and builder API.
- `src/eval.rs` — pure policy evaluation for both stages, including rule
  semantics and the complete-set caveat for `before_tool_batch`.
- `src/lib.rs` — middleware invocation, stage masking, and outcome mapping.

This crate is a T1 native adapter. It is not isolated.
