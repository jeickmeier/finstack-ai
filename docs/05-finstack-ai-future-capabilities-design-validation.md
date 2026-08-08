---
title: "finstack-ai Future Capabilities Design Validation"
subtitle: "Subagents, delegation, memory, knowledge retrieval, human workflows, and minimum-change microkernel recommendations"
author: "Project Draft"
date: "2026-08-08"
---

```{=openxml}
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
```

# Contents

Document control

Executive design verdict

1. Purpose, scope, and review method

2. Baseline architecture under review

3. Cross-cutting extension patterns

4. Capability accommodation summary

5. Subagents

6. Delegated work

7. Memory system

8. RAG and knowledge-base search

9. Workflow and human-in-the-loop interaction

10. Scheduled, background, and long-running work

11. Multi-agent teams and fan-out/fan-in

12. Sandboxed code execution and computer use

13. Multi-channel ingress and conversation routing

14. Guardrails, evaluations, and verification loops

15. Capability and bundle composition model

16. Reference bundles

17. Minimum core changes recommended

18. Proposed type and record changes

19. Implementation-plan impact

20. Conformance and future-proofing tests

21. Risks and architectural anti-patterns

22. Final decision and acceptance checklist

Appendix A. Feature-to-primitive traceability

Appendix B. Example bundle specifications

Appendix C. Glossary

```{=openxml}
<w:p><w:r><w:br w:type="page"/></w:r></w:p>
```

# Document control

| Field | Value |
|---|---|
| Product | finstack-ai |
| Document | Future Capabilities Design Validation |
| Version | 0.1 |
| Status | Draft for architecture validation and implementation planning |
| Date | 2026-08-08 |
| Primary audience | Maintainers, framework architects, implementation teams, extension authors, and AI coding agents |
| Related documents | Product Requirements Document v0.1; Architecture Specification v0.1; Technical Design v0.1; Implementation Plan v0.1 |

# Executive design verdict

The current **agent microkernel** direction is the correct foundation for the future capabilities examined in this document. The six existing ports—`Model`, `Toolset`, `ContextProvider`, `Middleware`, `JournalStore`, and `Observer`—are sufficient. None of the capabilities reviewed requires a seventh primary extension port, a generic workflow graph inside the kernel, or feature-specific kernel concepts for memory, retrieval, channels, code execution, or multi-agent products.

The design does, however, benefit from three small semantic refinements before the journal and public contracts freeze:

1. **General deferred-effect semantics.** Any model, tool, interaction, or externally hosted operation must be able to return a durable external handle, suspend the run, and later complete the same `EffectId` without inventing a feature-specific record path.
2. **Run lineage.** A run should carry optional parent/root relationship metadata so subagents and delegated agent work can propagate cancellation, deadlines, budget attribution, audit identity, and recovery correlation.
3. **Generalized interactions.** The proposed approval primitive should become an `Interaction` primitive. Approval remains a standard interaction profile, while the same mechanism supports forms, questions, choices, reviews, corrections, and externally assigned human tasks.

A fourth change is primarily a contract clarification rather than new machinery: the final behavior-changing middleware stage should be named or defined as **`before_finalize`**, making it explicit that verification and policy middleware may prevent terminal completion before the terminal record is committed.

Everything else belongs above or outside the kernel:

```text
agent microkernel
    universal run, effect, journal, event, and recovery semantics
            |
            v
runtime services
    agent invocation, scheduling, external completion, resource pools
            |
            v
extensions
    models, toolsets, context, middleware, stores, observers
            |
            v
capabilities and bundles
    subagents, memory, RAG, HITL workflows, coding, research, channels
```

The result is a design that remains concise while accommodating broad future products. The kernel gains only primitives whose meaning must remain identical across Rust, Python, browser WebAssembly, remote servers, and isolated components.

# 1. Purpose, scope, and review method

## 1.1 Purpose

This document stress-tests the proposed finstack-ai architecture against capabilities likely to be required after the initial release. It answers four questions for each capability:

1. Can it be implemented using the current kernel and six extension ports?
2. Which pieces should be capabilities, extensions, runtime services, applications, or isolated plugins?
3. Which durable states and effects must survive restart?
4. Where the design is insufficient, what is the smallest safe change to the microkernel?

The goal is not to fully specify each future product. The goal is to ensure that the foundational abstractions do not force a rewrite when those products arrive.

## 1.2 Capabilities reviewed

The requested capabilities are:

- subagents;
- delegated work;
- memory systems;
- RAG and knowledge-base searches; and
- workflow and human-in-the-loop interaction.

Five additional capabilities are included because they stress different architectural boundaries:

- scheduled, background, and long-running work;
- multi-agent teams with parallel fan-out and fan-in;
- sandboxed code execution and computer use;
- multi-channel ingress and conversation routing; and
- guardrails, evaluations, and verification loops.

## 1.3 Evaluation criteria

Each capability is evaluated against the following criteria.

| Criterion | Question |
|---|---|
| Kernel fit | Can universal semantics remain inside the existing deterministic run machine? |
| Port fit | Can implementations use one or more of the six current ports? |
| Durability | Can accepted work, suspension, completion, cancellation, and recovery be represented? |
| Composition | Can the feature be bundled without hidden ordering or dependency problems? |
| Native performance | Can the native path resolve once and use direct Rust calls? |
| Binding performance | Can Python and WASM use coarse calls and batched events? |
| Isolation | Can untrusted implementations move to WIT/process boundaries without changing semantics? |
| Scope discipline | Can the feature avoid introducing product-specific concepts into the kernel? |

## 1.4 Decision categories

Each capability receives one of four classifications:

- **Supported as designed:** no material architecture change.
- **Supported with runtime/SDK additions:** no kernel change, but a reusable service or composition type is needed.
- **Supported with a small kernel refinement:** the feature exposes a missing universal semantic primitive.
- **Not appropriate for the kernel:** implement as a higher-level workflow, product, or external service.

# 2. Baseline architecture under review

## 2.1 Kernel responsibility

The current design places the following universal semantics inside the kernel:

- canonical content, messages, and conversation entries;
- sessions, lanes, runs, turns, effects, calls, and stable identifiers;
- the canonical model/tool continuation loop;
- deterministic decisions and record application;
- journal record definitions and validation;
- event ordering;
- cancellation, limits, suspension, and recovery rules;
- capability activation state; and
- valid-history construction.

The kernel performs no external I/O and depends on no async runtime.

## 2.2 Existing extension ports

| Port | Primary purpose | Typical future uses |
|---|---|---|
| `Model` | Provider-neutral model request and stream | Parent and child agents; planning; summarization; multimodal models |
| `Toolset` | Catalog and dispatch of related tools | Delegation, memory writes, RAG search, sandboxes, channel actions |
| `ContextProvider` | Budgeted context contribution | Memory recall, RAG, user profile, repository context, reminders |
| `Middleware` | Limited behavior-changing stages | Approvals, guardrails, compaction, verification, budget policy |
| `JournalStore` | Persist kernel-defined records | In-memory, SQLite, PostgreSQL, IndexedDB, workflow-backed stores |
| `Observer` | Immutable event consumption | Audit, tracing, billing, offline evaluation, analytics |

## 2.3 Existing composition primitives

The current design already provides:

- immutable `AgentSpec` and `ResolvedAgent`;
- typed registration and namespaced component references;
- declarative `CapabilitySpec`;
- durable capability activation;
- immutable conversation trees and multiple lanes;
- stable `EffectId` values and commit-before-effect ordering;
- approval, timer, cancellation, and suspension concepts;
- large-payload `BlobRef` values; and
- native, Python, browser-WASM, remote-server, and WIT component deployment modes.

## 2.4 Governing design rule

A feature belongs in the kernel only when all correct implementations must share the same state transition, journal, event-ordering, cancellation, or recovery meaning.

Implementation-specific work stays outside:

```text
kernel owns                    extension/runtime owns
----------------------------  --------------------------------------
an effect is requested         how the provider or tool performs it
an interaction is pending      how a UI routes and renders it
a child run has a parent       which subagent is selected
context has a budget           which index or memory store is searched
work is suspended              which queue, worker, or human completes it
```

# 3. Cross-cutting extension patterns

The reviewed capabilities repeatedly use the same small set of patterns. Standardizing these patterns is more important than adding feature-specific interfaces.

## 3.1 Capability, extension, bundle, and agent

These terms should remain distinct.

| Term | Meaning |
|---|---|
| Extension | Executable code that registers implementations of existing ports |
| Capability | Declarative, optionally model-visible behavior composed from instructions, toolsets, context providers, and middleware |
| Bundle | Deployment/application packaging that selects agents, capabilities, components, configuration, and defaults |
| Agent | An immutable resolved definition used to execute runs |
| Plugin | An independently distributed extension, usually isolated through WIT or a process |

A memory system is usually an extension plus one or more capabilities. A research assistant is a bundle containing an agent, memory and retrieval capabilities, subagent definitions, and policies.

## 3.2 Nested agent invocation

Subagents and multi-agent teams require one agent to invoke another without making agent invocation a seventh extension port.

The recommended runtime service is:

```rust
pub trait AgentInvoker: Send + Sync {
    fn start_child(
        &self,
        ctx: ChildRunContext,
        request: ChildRunRequest,
    ) -> BoxFuture<'static, Result<ChildRunHandle, AgentInvokeError>>;
}
```

`AgentInvoker` is a host/runtime service, not an implementation family. It starts the same kernel run machine with another `ResolvedAgent` and a durable run relationship. A built-in `AgentToolset` can expose selected agents to a parent model as ordinary tools.

## 3.3 Deferred external completion

Several future features cannot complete during the original process lifetime:

- a remote research job;
- a human task;
- a background provider response;
- an external worker;
- a long-running sandbox job; or
- a delegated agent hosted elsewhere.

These should share one effect lifecycle:

```text
EffectRequested
      |
      v
EffectDeferred(external handle, wake policy, expiry)
      |
      +---- process may stop ----+
      |                          |
      v                          v
external callback          runtime reconciliation/poll
      |                          |
      +------------+-------------+
                   v
          EffectCompleted or terminal failure
```

The same `EffectId` remains authoritative from request through completion. Feature-specific systems may retain richer status in their own stores, but the kernel only needs requested, deferred, completed, failed, cancelled, and uncertain states.

## 3.4 Human and external interactions

A yes/no approval is only one form of external input. The durable primitive should support:

- approve or deny;
- choose one or more options;
- enter free text;
- complete a typed form;
- review and edit a proposed artifact;
- request clarification;
- acknowledge a warning; and
- assign or delegate a task.

The kernel should own pending/resolved/cancelled/expired state. Presentation, identity resolution, notifications, escalation, and inboxes remain outside.

## 3.5 Artifacts, sources, and provenance

Memory, RAG, delegation, sandboxing, and multi-agent work all produce large or attributable results. The existing `BlobRef` mechanism should be paired with a reusable non-kernel `ArtifactRef` metadata shape:

```text
ArtifactRef
  blob_ref
  logical type
  title/name
  producer run/effect/tool
  source/provenance references
  content digest
  access classification
  optional version and parent artifact
```

The kernel continues to persist references, not large payloads. Artifact stores remain runtime/application services.

## 3.6 Identity, cancellation, deadlines, and budgets

Every nested or externally completed operation should receive an explicit invocation context:

- root, parent, and current run IDs;
- parent effect ID when applicable;
- session and lane IDs;
- principal and tenant identity;
- cancellation token and deadline;
- trace/correlation metadata;
- budget scope and allocation; and
- idempotency/effect key.

Cancellation propagates down the run relationship by default. Detached work must be explicitly requested and audited.

Budget aggregation should initially be a runtime service. The kernel continues to enforce per-run limits and records usage. A `BudgetScopeId` allows a parent and child runs to be aggregated without placing pricing or shared-ledger policy in the kernel.

# 4. Capability accommodation summary

| Capability | Fit | Main building blocks | Smallest required change |
|---|---|---|---|
| Subagents | Strong | `AgentInvoker`, `AgentToolset`, lanes, child runs | Add run lineage metadata |
| Delegated work | Strong after refinement | Toolset, deferred effect, interaction, external worker | General deferred-effect lifecycle |
| Memory system | Strong | Context provider, toolset, middleware, observer, private store | None |
| RAG / knowledge search | Strong | Context provider, search toolset, blob/provenance | None |
| Workflow HITL | Strong after refinement | External workflow, interactions, timers, suspension | Generalize approval to interaction |
| Background/long-running work | Strong after refinement | Server/scheduler, timers, deferred effects | General deferred-effect lifecycle |
| Multi-agent teams | Strong | Child runs, lanes, runtime fan-out, artifacts | Run lineage metadata |
| Sandboxed code/computer use | Strong | Toolset, WIT/process isolation, blobs, progress | None |
| Multi-channel routing | Strong | Application adapters, session server, lanes, principals | None |
| Guardrails/evaluations/verification | Strong with clarification | Middleware, observer, toolsets | Clarify `before_finalize` stage |

The conclusion is deliberately conservative: no feature-specific port or generic workflow engine is needed in the kernel.

# 5. Subagents

## 5.1 Intended behavior

A subagent is a named agent definition invoked by another agent or application to perform bounded work. Common forms include:

- a researcher asked to gather sources;
- a reviewer asked to inspect code or documents;
- a summarizer asked to compress multiple reports;
- a specialist model with a restricted toolset;
- a cheaper model handling routine subtasks; and
- an isolated agent operating on a separate lane or workspace.

A subagent may be synchronous, parallel, durable, remotely hosted, or detached. The first implementation should prioritize synchronous and parallel child runs, with durable parent-child correlation.

## 5.2 Recommended composition

```text
Subagent capability bundle
  AgentCatalog entries
    researcher -> ResolvedAgent
    reviewer   -> ResolvedAgent
    summarizer -> ResolvedAgent
  AgentToolset
    delegate_to_researcher
    delegate_to_reviewer
    delegate_to_summarizer
  optional delegation middleware
    depth, cost, tool, tenant, or approval policy
```

`AgentCatalog` and `AgentInvoker` belong in `finstack-ai-sdk`/`finstack-ai-runtime`. They are not kernel ports because they invoke the same framework rather than supply an external implementation of a kernel operation.

## 5.3 Execution sequence

```text
parent model emits delegate_to_researcher(task)
    |
    v
parent tool effect committed with EffectId E
    |
    v
AgentToolset derives child RunId from E and requests AgentInvoker
    |
    v
child lane/session and RunAccepted committed with parent relation
    |
    v
child executes normal kernel loop
    |
    v
child terminal result and usage returned to AgentToolset
    |
    v
parent tool completion committed and parent continues
```

The deterministic child ID prevents duplicate child runs if the parent tool invocation is retried after a crash.

## 5.4 Session and lane choices

Three placement modes are useful:

| Mode | Use | Trade-off |
|---|---|---|
| New lane in same session | Shared conversation prefix, parallel specialist work | Requires lane-safe access and compatible session policy |
| New child session | Strong isolation, different agent/tool/store policies | Context must be passed by reference or copied intentionally |
| Remote child session | Independent service or tenant boundary | Requires remote protocol and deferred completion |

The default should be a new lane anchored to the parent lane leaf when the store and agent policies are compatible. A new session is safer when tool permissions, tenancy, retention, or storage differ.

## 5.5 Durability and recovery

The parent tool effect and child run must not become two unrelated journals. The child `RunAccepted` record should include:

- `root_run_id`;
- `parent_run_id`;
- `parent_effect_id`;
- relation kind such as `ChildAgent`;
- depth;
- optional budget scope; and
- optional caller-supplied delegation ID.

On recovery:

1. If the child completed, return its existing terminal result.
2. If the child is active or suspended, reconnect or resume it.
3. If the child was never accepted, create it using the deterministic derived ID.
4. If the child has an uncertain non-repeatable state, propagate suspension to the parent.

## 5.6 Cancellation, limits, and recursion

The runtime should enforce:

- maximum child depth;
- maximum total child runs per root run;
- maximum concurrent children;
- parent/child deadline intersection;
- inherited or reduced tool permissions;
- budget allocation; and
- cancellation propagation.

These are runtime policies expressed through `ChildRunRequest` and middleware. They do not require kernel-specific subagent logic beyond relationship metadata.

## 5.7 Python and WASM behavior

Rust-backed subagents remain entirely native even when the parent is controlled from Python. Python receives batched child lifecycle events only if requested.

A Python-defined child agent can be registered in the catalog, but Python callbacks occur at its model/tool boundaries in the normal way. The parent does not repeatedly cross the FFI boundary for child scheduling.

Browser WASM can support child runs in a worker-backed runtime. Child models/tools are host adapters, while run state remains inside WASM. Concurrency limits should reflect browser worker and connection limits.

## 5.8 Design verdict

**Supported with a runtime/SDK addition and one small kernel refinement.** Add run lineage metadata. Do not add a `Subagent` port or a separate subagent state machine.

# 6. Delegated work

## 6.1 Intended behavior

Delegated work is broader than subagents. The executor may be:

- another finstack-ai agent;
- a human analyst;
- a remote service;
- a queue worker;
- a hosted research API;
- an enterprise workflow; or
- an external organization.

Delegation may wait for completion, return a handle immediately, or continue in the background.

## 6.2 Work-item model outside the kernel

A reusable delegation extension can own a `WorkItem` domain model:

```text
WorkItem
  work_item_id
  parent effect/run
  executor type and target
  task payload
  status
  assignee
  external handle
  result artifact(s)
  created/updated/expiry
  policy and audit metadata
```

This richer state belongs in the delegation service or workflow store. The kernel records the effect handle and final normalized outcome.

## 6.3 Toolset design

A delegation toolset may expose:

- `delegate_task`;
- `get_task_status`;
- `await_task`;
- `cancel_task`;
- `list_delegated_tasks`; and
- `collect_task_result`.

Two interaction styles should be supported.

### Handle-returning delegation

`delegate_task` creates a work item and immediately returns a stable reference. The parent model may continue and poll later. This is an ordinary completed tool effect and requires no kernel change.

### Awaiting delegation

`delegate_task` creates the work item, returns a deferred external handle to the runtime, and suspends the parent until the same effect completes. This requires general deferred-effect semantics.

## 6.4 External completion

The runtime should expose an authenticated API similar to:

```rust
pub async fn complete_external_effect(
    effect_id: EffectId,
    completion: ExternalEffectCompletion,
    principal: Principal,
) -> Result<CompletionReceipt>;
```

The runtime validates:

- effect remains outstanding;
- completion principal is authorized;
- response matches the registered output schema;
- completion is not expired or cancelled;
- duplicate completion matches the existing digest; and
- lane/session ownership is preserved.

The kernel then applies a normal completion record and resumes from the existing checkpoint.

## 6.5 Human delegation

Human work should use the generalized interaction mechanism when the expected response is directly consumed by the run. A separate work-item service is appropriate when the human task has assignment, queues, service-level objectives, attachments, collaboration, or a lifecycle beyond one interaction.

## 6.6 Failure and uncertainty

Delegated work must distinguish:

- rejected before dispatch;
- accepted by executor;
- pending;
- completed;
- failed;
- cancelled;
- expired;
- result disputed; and
- externally uncertain.

The kernel needs only a normalized final/deferred/uncertain status. The extension retains detailed work-item history.

## 6.7 Design verdict

**Supported after adding a general deferred-effect lifecycle.** Do not add delegation-specific journal records or a `Delegation` port.

# 7. Memory system

## 7.1 Memory taxonomy

“Memory” should not be represented as one framework object. Different forms have different consistency and security needs.

| Memory type | Meaning | Recommended implementation |
|---|---|---|
| Working memory | Current run state and recent context | Kernel run/session state |
| Conversation memory | Durable transcript and branches | Conversation tree and `JournalStore` |
| Episodic memory | Recalled prior events or summaries | `ContextProvider` plus memory store |
| Semantic memory | Facts and embeddings extracted across sessions | `ContextProvider` and search toolset |
| Procedural memory | Skills, instructions, standard methods | Declarative capabilities |
| User/profile memory | Preferences, identity, constraints | Scoped `ContextProvider` and management tools |

The authoritative journal must not silently become the long-term semantic memory database. Journal stores preserve framework truth; memory extensions preserve derived or application-owned knowledge.

## 7.2 Recommended memory extension

```text
MemoryExtension
  MemoryContextProvider
    recalls relevant facts/episodes before model requests
  MemoryToolset
    remember, search_memory, forget, correct, inspect
  MemoryCaptureMiddleware
    extracts candidate memories at safe stages
  MemoryObserver (optional)
    asynchronous analytics or low-criticality extraction
  private MemoryStore
    vector, relational, graph, or remote implementation
```

The extension may register several capabilities:

- `memory.read`;
- `memory.write`;
- `memory.manage`;
- `memory.consolidate`; and
- `memory.profile`.

This allows applications to expose recall without allowing autonomous writes or deletion.

## 7.3 Recall path

The normal automatic recall path is a `ContextProvider`:

```text
prepare_context
  -> MemoryContextProvider receives user input, recent history, scope, budget
  -> retrieve candidates
  -> rank, deduplicate, and redact
  -> return ContextItems with provenance and token estimates
  -> runtime budgets and orders contributions
```

The provider must never mutate conversation history directly.

Model-driven recall uses a `search_memory` tool when the model needs iterative or scoped search. Automatic context should remain small and high-confidence; deep exploration belongs in tools.

## 7.4 Memory write paths

Three write modes are useful.

### Explicit model/user write

A `remember` tool validates and stores a memory. It uses the tool effect ID as an idempotency key.

### Post-run durable capture

`before_finalize` or another safe middleware stage identifies candidate memories and performs an idempotent external write under a committed middleware effect. The normalized capture outcome is recorded before terminal completion when application policy requires the memory to be durable.

### Eventual asynchronous capture

An observer consumes terminal events and writes memory asynchronously. This is appropriate when memory loss does not invalidate the run. Observer failure must not change the completed result.

## 7.5 Correction, deletion, and provenance

A production memory system needs:

- tenant, user, agent, and workspace scopes;
- source run/message references;
- confidence and extraction method;
- creation and last-confirmed time;
- supersession and correction links;
- retention and decay policies;
- consent and sensitivity classification;
- deletion/tombstone semantics; and
- explainable retrieval evidence.

These remain memory-domain concerns. The kernel persists only any references included in context or tool results.

## 7.6 Cache stability

Automatic recall can destroy provider prompt-cache reuse if inserted unpredictably. The memory capability should support:

- stable ordering by memory ID after ranking tiers;
- a bounded stable recall prefix;
- cache keys on context contributions;
- checkpoint-only activation changes; and
- explicit policy for dynamic, volatile memories.

The existing `ContextContribution.cache_key` and deterministic provider order are sufficient.

## 7.7 Python and WASM behavior

Rust-backed memory stores and retrieval remain native in Python deployments. Python memory providers can implement the same coarse `collect` call.

Browser memory can use IndexedDB or a remote service. Embedding generation is typically a host/model effect. Large memory attachments use `BlobRef` values.

## 7.8 Design verdict

**Supported as designed.** No `Memory` kernel port is needed. Memory is a composition of context, tools, middleware/observation, and an extension-owned store.

# 8. RAG and knowledge-base search

## 8.1 Separate ingestion from retrieval

A knowledge system has two major planes:

```text
ingestion/indexing plane             agent retrieval plane
-------------------------------     ------------------------------
connectors and crawlers              automatic context provider
parsing and chunking                 model-driven search toolset
embedding and indexing               source/blob resolution
ACL and metadata sync                citations and provenance
```

The ingestion plane is an application or workflow built around finstack-ai, not part of the agent kernel.

## 8.2 Dual retrieval modes

### Automatic retrieval

A `KnowledgeContextProvider` contributes a small, budgeted set of high-value excerpts before the model request.

### Interactive retrieval

A `KnowledgeSearchToolset` exposes search, fetch, browse, related-document, and citation-resolution tools. It is preferable for broad exploration, multi-query research, or when the model needs to inspect full documents.

Both may share the same index client and authorization layer.

## 8.3 Context and citation shape

Each retrieved item should include:

- source identifier and canonical URI/reference;
- title and document type;
- excerpt or `BlobRef`;
- retrieval score and rank;
- source timestamps/version;
- tenant and access scope;
- page/section/chunk location;
- digest; and
- citation label stable within the run.

The existing requirement that external context carry provenance is sufficient. Citation rendering is a product/UI concern, while source references remain in normalized content metadata.

## 8.4 Knowledge-base authorization

The retrieval extension must receive principal and tenant context. Authorization is enforced before excerpts leave the provider/toolset. The model cannot request a broader scope than the application granted.

For isolated plugins, host permissions should restrict network endpoints and configuration/secrets. Document-level ACLs remain the extension’s responsibility.

## 8.5 Long documents and media

Full documents, images, audio, and tables should be externalized as blobs. Search results return short excerpts and references. Tools can resolve a selected reference into text, structured sections, or media blocks subject to budgets.

## 8.6 Index freshness and reproducibility

For auditable runs, context items should record the index version or source digest used. Replaying a historical run need not reproduce a changing index query unless policy requires it; the recorded context contribution is the authoritative input to that completed turn.

## 8.7 Design verdict

**Supported as designed.** RAG is a context provider plus a toolset, with ingestion outside the kernel. No retrieval-specific core type is required beyond provenance and blob references already planned.

# 9. Workflow and human-in-the-loop interaction

## 9.1 Two layers of workflow

The framework should distinguish an **agent run** from an **application workflow**.

```text
agent run
  canonical model/tool continuation with suspension and recovery

application workflow
  several runs, deterministic tasks, branches, timers, approvals,
  external systems, compensation, and business state
```

The kernel should not become a general Temporal-like graph engine. External workflows compose runs through the SDK, server protocol, or durable-runtime adapter.

## 9.2 Human interaction within a run

A run may need:

- approval before a tool;
- clarification from the user;
- selection among options;
- typed data collection;
- document or plan review;
- correction of extracted fields;
- acceptance of a result; or
- escalation to another role.

The current approval-specific concept is too narrow. It should become a generalized `Interaction` effect.

## 9.3 Interaction lifecycle

```text
InteractionRequested
  prompt/content blocks
  response schema
  interaction kind
  assignee/role hints
  expiry and delegation policy
        |
        v
RunSuspended(awaiting interaction)
        |
        +---- UI/inbox/channel/workflow routes request
        |
        v
InteractionResolved / Rejected / Expired / Cancelled
        |
        v
validated response becomes normalized kernel input
        |
        v
run resumes at recorded continuation
```

Approval is represented by a response schema such as `{ decision: approve|deny, comment?: string }` and an approval profile. The kernel does not render forms or choose assignees.

## 9.4 Workflow-engine integration

A workflow adapter should drive the same kernel effects rather than wrap an entire opaque agent call. Useful integration points include:

- durable sleep/timers;
- persistence handoff;
- external effect dispatch;
- interaction waiting;
- run resume;
- cancellation; and
- effect-ID-based retry/idempotency.

The workflow engine may be authoritative for business workflow state while the kernel journal remains authoritative for agent-run semantics.

## 9.5 Long waits and schema evolution

Interaction requests must be self-describing and versioned because they may remain open across deployments. The response schema, capability version, and prompt payload digest should be stored with the request.

When an application upgrades while an interaction is pending, it must either continue accepting the recorded schema or migrate/cancel the request explicitly.

## 9.6 Human task inboxes

Assignment, reminders, delegation, queues, comments, and service-level objectives belong in an application or work-item service. The kernel needs only:

- interaction identity;
- request payload/schema;
- state;
- resolution principal;
- response;
- expiry/cancellation; and
- continuation correlation.

## 9.7 Design verdict

**Supported with one small kernel refinement.** Generalize approval to interaction. Keep workflow graphs, inboxes, notifications, and business state outside the kernel.

# 10. Scheduled, background, and long-running work

## 10.1 Capability forms

Future products may need:

- a run scheduled for a future time;
- recurring research or monitoring;
- a detached background run;
- a model/provider background response;
- a long-running tool or sandbox job;
- waiting for external data; and
- resuming when a webhook arrives.

## 10.2 Scheduling boundary

A scheduler or cron service is not a kernel port. It creates or resumes runs through public APIs.

```text
Scheduler
  schedule definition and recurrence
  next-fire calculation
  ownership and leasing
  missed-run policy
  -> starts finstack-ai runs with idempotency keys
```

The kernel’s timer effect remains useful for waits *inside* an accepted run, such as retries, rate-limit backoff, interaction expiry, or polling intervals.

## 10.3 Detached runs

A detached run should have:

- an owning session/lane;
- durable `RunAccepted` state;
- consumer-independent cancellation policy;
- observable status and terminal result;
- retention policy; and
- an application-level delivery target.

The remote session server is a natural host. Client disconnection does not cancel durable work unless explicitly configured.

## 10.4 Long-running external jobs

A model or tool may return a deferred handle. The runtime can choose callback, polling, or workflow-driven wakeup. The kernel is independent of that choice.

A polling runtime should persist the next wake time through timer effects rather than busy-waiting. Duplicate callbacks or polls must settle against the same `EffectId`.

## 10.5 Recurring work and memory

Recurring runs should normally use new run IDs and explicit shared memory or artifacts. They should not mutate one never-ending conversation indefinitely. A monitoring bundle may store recent summaries in semantic memory and retain full runs under normal retention policy.

## 10.6 Design verdict

**Supported after general deferred-effect semantics.** Scheduling and recurrence remain server/application responsibilities. Do not add cron or a job scheduler to the kernel.

# 11. Multi-agent teams and fan-out/fan-in

## 11.1 Intended behavior

A team pattern coordinates several agents, often in parallel:

- multiple researchers gather independent evidence;
- specialist reviewers inspect different dimensions;
- a debate/critique round compares proposals;
- an adjudicator selects or synthesizes results; and
- a coordinator repeats selected work until quality criteria are met.

This is more than one subagent call, but it should still compose ordinary child runs.

## 11.2 Orchestration options

| Option | Best use |
|---|---|
| Parent model issues several agent tools | Dynamic, small fan-out controlled by model |
| Deterministic runtime coordinator | Fixed parallel pattern with strict budgets |
| External workflow engine | Long-running, branch-heavy, human-involved team workflow |
| Sandboxed code-mode coordinator | Model writes one bounded program invoking several agents/tools |

None requires a new run loop in the kernel.

## 11.3 Fan-out/fan-in service

A reusable SDK component can offer:

```rust
pub async fn invoke_many(
    invoker: &dyn AgentInvoker,
    requests: Vec<ChildRunRequest>,
    policy: FanOutPolicy,
) -> Result<FanInResult>;
```

Policy controls concurrency, fail-fast versus collect-all behavior, deadlines, quorum, and result ordering. Each child remains a first-class run with lineage.

## 11.4 Result transport

Child results should normally return:

- a concise textual or structured summary;
- usage and status;
- artifact references; and
- source/provenance references.

Large child transcripts should not be injected into the parent context automatically. The parent may inspect them through tools or artifacts.

## 11.5 Shared state

Agents should not share mutable in-memory scratch state. Coordination uses:

- immutable shared conversation prefixes;
- explicit artifacts;
- memory/knowledge services;
- workflow state; or
- typed messages/results.

This preserves deterministic replay and avoids hidden cross-lane races.

## 11.6 Budget and cancellation

A `BudgetScopeId` can group child usage. Runtime policy may reserve budget per child, stop remaining children when a threshold is reached, or allow a coordinator to reallocate unused budget.

Root cancellation cascades to active children. A detached child requires an explicit policy and changes its owning relationship.

## 11.7 Design verdict

**Supported by subagent primitives plus runtime orchestration.** Run lineage is the only kernel refinement. Do not add “swarm,” “team,” or consensus concepts to the kernel.

# 12. Sandboxed code execution and computer use

## 12.1 Capability forms

This category includes:

- shell execution;
- Python or JavaScript code mode;
- remote containers or micro-VMs;
- browser automation;
- desktop/computer use;
- notebook execution; and
- secure execution of tool-composition programs.

## 12.2 Toolset boundary

All are toolsets from the agent’s perspective. A tool call may start a local process, invoke a remote sandbox, or execute a WIT component. The kernel sees a normal tool effect and progress/completion events.

A code-mode capability can expose one `run_code` tool that runs a restricted program allowed to call selected host tools. The sandbox host enforces:

- callable tool allowlist;
- CPU, memory, time, and output limits;
- filesystem/network capabilities;
- secret isolation;
- recursive-call depth;
- artifact quotas; and
- cancellation.

## 12.3 Isolation modes

| Mode | Trust and performance |
|---|---|
| Native in-process | Fastest; trusted implementation only |
| Child process/container | Stronger OS isolation; good for languages and shells |
| WASI component | Portable capability isolation; coarse host calls |
| Remote sandbox | Strong operational isolation; network latency and external lifecycle |

The selected mode does not change kernel semantics.

## 12.4 Computer-use streaming

Screenshots and video frames should be blobs or compressed references. High-frequency progress is transient and coalesced. Only selected observations/actions and the final result need durable representation unless product policy requests an audit recording.

## 12.5 Long-running execution

Remote sandboxes may return deferred handles. The general effect-deferral mechanism supports polling or callbacks without introducing a sandbox-specific kernel record.

## 12.6 Design verdict

**Supported as designed**, with deferred effects needed only for externally hosted long-running jobs. No code-execution or browser concepts belong in the kernel.

# 13. Multi-channel ingress and conversation routing

## 13.1 Intended behavior

Applications may receive messages from:

- CLI/TUI;
- web or mobile clients;
- Slack, Teams, Discord, Telegram, or email;
- webhooks and event streams;
- voice systems; and
- embedded product surfaces.

Channels are applications/adapters around the session API, not kernel extensions.

## 13.2 Routing model

```text
external channel event
  -> authenticate and normalize principal
  -> deduplicate channel event
  -> resolve agent + session + lane
  -> append user input/start or steer run
  -> subscribe to event batches
  -> render/deliver output through channel adapter
```

A stable mapping usually uses:

```text
(channel account, conversation/thread identity) -> (session_id, lane_id)
```

The lane model already supports independent threads sharing an agent and, where desired, a history prefix.

## 13.3 Ingress deduplication

Channel delivery retries should be deduplicated at the server/adapter boundary using the external message ID. The run API may accept an optional idempotency key, but channel protocol details should not enter the kernel.

## 13.4 Principal and authorization

The normalized `Principal` flows into run, tool, context, and interaction calls. Toolsets and retrieval providers enforce the granted tenant/user scope. Human interactions can be routed back to the originating channel or to a separate approval inbox.

## 13.5 Delivery semantics

Channels vary in streaming support. The adapter can:

- stream edits/drafts;
- buffer until sentence or message boundaries;
- split large results;
- deliver artifact links; or
- send a final result only.

These are presentation policies over the immutable event stream.

## 13.6 Design verdict

**Supported as designed.** Use the remote session protocol, principals, sessions, and lanes. Do not add channel traits or vendor integrations to the kernel.

# 14. Guardrails, evaluations, and verification loops

## 14.1 Three distinct concerns

| Concern | Mechanism |
|---|---|
| Prevent or modify unsafe behavior | Behavior-changing middleware |
| Require evidence before completion | `before_finalize` middleware plus tools/context |
| Measure quality without altering execution | Observer and offline evaluation tooling |

Combining all three under one “guardrail” abstraction creates confusing replay and failure semantics.

## 14.2 Policy middleware

Middleware can:

- filter tools;
- require interaction/approval;
- redact or transform requests;
- enforce cost or domain limits;
- reject outputs;
- request retry or continuation; and
- fail or suspend runs.

Durable outcomes are recorded so recovery does not silently rerun non-replay-safe policy.

## 14.3 Verification loops

Examples include:

- require tests after code changes;
- require source citations for research;
- require reconciliation of conflicting subagent reports;
- require fresh data before finalizing a financial report; and
- require structured validation of an artifact.

The final behavior-changing stage must occur before the terminal record. Recommended semantics:

```text
candidate final result
  -> before_finalize middleware
       continue with final result
       request another model turn with feedback
       request interaction
       fail or suspend
  -> terminal record only after middleware accepts
```

The current `after_run` name is ambiguous. It should be replaced by or explicitly defined as `before_finalize` for behavior-changing middleware. Post-terminal work belongs to observers.

## 14.4 Evaluations

Observers can capture traces and artifacts for offline evaluation. Test/evaluation packages may replay recorded model/tool fixtures, score outputs, compare versions, or run agentic evaluators. They must not become part of the production kernel.

## 14.5 Design verdict

**Supported with a contract clarification.** Define a pre-terminal `before_finalize` stage. No evaluation or guardrail port is needed.

# 15. Capability and bundle composition model

## 15.1 Why bundles are needed

`CapabilitySpec` describes reusable agent behavior, but an application also needs to package:

- several agent definitions;
- required component implementations;
- multiple capabilities;
- default activation and policies;
- binding/application configuration;
- optional server/channel components; and
- compatibility constraints.

That unit should be a non-kernel `BundleSpec`.

## 15.2 Proposed `BundleSpec`

```rust
pub struct BundleSpec {
    pub schema_version: u16,
    pub id: BundleId,
    pub version: Version,
    pub agents: Vec<AgentSpec>,
    pub capabilities: Vec<CapabilitySpec>,
    pub required_components: Vec<ComponentRequirement>,
    pub optional_components: Vec<ComponentRequirement>,
    pub defaults: BundleDefaults,
    pub config_schema: Option<SchemaRef>,
    pub compatibility: CompatibilityRequirements,
}
```

The bundle references implementations; it does not contain native handles or execute code.

## 15.3 Dependency types

Keep dependency resolution narrow and explicit:

- required component ID and compatible major version;
- optional component;
- required capability;
- one-of component alternatives;
- conflicts;
- required host feature such as browser storage or isolated plugin support; and
- minimum framework contract version.

Do not build a general-purpose package solver into the runtime. Package installation produces a lock/resolution file; agent construction consumes resolved registrations.

## 15.4 Configuration layering

Recommended precedence:

```text
component defaults
  < bundle defaults
  < application configuration
  < agent-specific configuration
  < run overrides explicitly permitted by policy
```

Secrets remain references resolved by the host. Bundles must not embed credentials.

## 15.5 Capability activation and dependencies

A capability may declare:

- always active;
- application-activated;
- model-loadable;
- disabled by default;
- required companion capabilities; and
- conflicting capabilities.

Activation remains durable lane state and occurs at checkpoints. The resolver expands capability references into pre-resolved toolsets, context providers, middleware, and instructions.

## 15.6 Middleware ordering

Bundles may include middleware, but the resolver still produces one globally ordered middleware chain per resolved agent. Conflicts and cycles fail agent construction. Bundle nesting must not create hidden wrapper order.

## 15.7 Bundle lifecycle

Installation and resolution occur outside the run path:

```text
package/bundle selection
  -> resolve component versions
  -> verify signatures and policy
  -> produce lock file
  -> register implementations
  -> validate BundleSpec/AgentSpec
  -> produce immutable ResolvedAgent values
  -> execute direct calls during runs
```

# 16. Reference bundles

## 16.1 Research and knowledge assistant

```text
Agents
  coordinator
  researcher
  verifier

Capabilities
  knowledge.auto_context
  knowledge.search
  memory.read
  memory.write-on-confirmation
  subagents.research
  verification.citations

Components
  primary model
  cheaper researcher model
  KnowledgeContextProvider
  KnowledgeSearchToolset
  MemoryContextProvider
  MemoryToolset
  AgentToolset
  citation/verification middleware
  journal store and observers
```

Flow:

1. Coordinator receives question.
2. Automatic retrieval supplies high-confidence context.
3. Coordinator delegates broad searches to parallel researcher child runs.
4. Research results return artifacts and citations rather than full transcripts.
5. Verifier checks source coverage and conflicts before finalization.
6. Confirmed durable facts may be written to memory.

Kernel needs: ordinary model/tool loop, child run lineage, effects, context, `before_finalize`, and journals.

## 16.2 Enterprise knowledge worker with approvals

```text
Capabilities
  enterprise_kb
  user_profile_memory
  document_generation
  human_review
  action_approval
  audit
```

Flow:

1. RAG retrieves tenant-authorized sources.
2. Agent drafts a report or action plan.
3. `before_finalize` requires a typed human review interaction.
4. User edits fields or returns comments.
5. Agent revises and requests final approval.
6. Approved external actions run through protected toolsets.
7. Observer writes immutable audit events.

Kernel needs: generalized interactions and existing middleware/effect semantics.

## 16.3 Coding agent

```text
Agents
  coder
  reviewer
  test investigator

Capabilities
  repository_context
  filesystem
  shell_or_sandbox
  subagent_review
  verification_tests
  memory.project_conventions
```

Flow:

- repository context enters through a provider;
- filesystem and code execution are toolsets;
- reviewer runs on a child lane;
- large diffs/test logs become artifacts;
- verification middleware prevents finalization without fresh test evidence; and
- risky operations request interaction approval.

No coding-specific kernel changes are required.

## 16.4 Investment or due-diligence workflow

```text
External workflow
  intake
  document ingestion/indexing
  specialist agent fan-out
  conflict analysis
  human analyst review
  committee approval
  final report and evidence package

finstack-ai runs
  extraction agents
  legal/credit/industry specialists
  synthesis agent
  verifier
```

The workflow engine owns business stages and deadlines. Each agent run has its own journal and lineage. Human reviews use interactions. Shared evidence uses artifacts and the knowledge store.

## 16.5 Multi-channel support agent

```text
Channel adapters
  web
  email
  Slack/Teams

Session model
  customer/account session
  lane per external thread

Capabilities
  customer profile memory
  support knowledge RAG
  ticket/action tools
  escalation interaction
  quality guardrails
```

Channels map identities to lanes. Memory and knowledge providers receive the normalized principal. Escalations either create interactions or delegated work items.

# 17. Minimum core changes recommended

## 17.1 KC-01: General deferred-effect lifecycle

**Priority:** Required before journal v1 freezes.

### Problem

The PRD expects suspension on external tool completion and deferred model responses, but the technical record/type sketches do not yet define a first-class deferred outcome for every effect.

### Change

Add a generic deferred record/state and external completion input. The same mechanism applies to model, tool, interaction, and extension-hosted effects.

```text
EffectRequested
EffectDeferred
EffectCompleted | EffectFailed | EffectCancelled
```

`EffectDeferred` contains an opaque external handle reference, optional next-poll time, expiry, reconciliation strategy, and output-schema digest. Secret resume credentials are stored by the host, not in public journal payloads.

### Why it belongs in the kernel

Suspension, duplicate completion, cancellation, event ordering, and recovery must be identical across all bindings and effect implementations.

### What it avoids

- delegation-specific records;
- provider-specific suspended states;
- workflow-specific callbacks;
- sandbox-job special cases; and
- ad hoc long-poll loops.

## 17.2 KC-02: Run lineage metadata

**Priority:** Recommended before session/lane journal contracts freeze.

### Problem

Lanes can represent subagents, but run records do not explicitly relate parent and child runs.

### Change

Add optional `RunRelation` to `RunAccepted` and public run metadata.

```text
root_run_id
parent_run_id
parent_effect_id
relation kind
nesting depth
optional budget scope
optional delegation/work-item reference
```

### Why it belongs in the kernel

Lineage affects audit correlation, cancellation trees, durable recovery, and interpretation of nested usage. It must survive process boundaries and bindings.

### What remains outside

Agent selection, fan-out policy, child result aggregation, and shared budget enforcement remain runtime/SDK concerns.

## 17.3 KC-03: Generalize approval to interaction

**Priority:** Recommended before PR-044 is implemented.

### Problem

A yes/no approval primitive cannot cleanly represent typed questions, review/edit cycles, selection, clarification, or externally assigned human tasks.

### Change

Replace approval-specific durable records/effect kind with a general `Interaction` request/resolution. Provide `ApprovalInteraction` as a built-in schema/profile.

### Why it belongs in the kernel

Pending external input, expiry, cancellation, authorization, duplicate resolution, and resumption are universal state-machine semantics.

### What remains outside

UI rendering, inboxes, assignee lookup, notifications, reminders, delegation, and escalation remain application services.

## 17.4 KC-04: Clarify the final middleware stage

**Priority:** Required contract clarification; minimal or no data-model change.

### Problem

`after_run` may imply that terminal state has already committed, which would prevent verification or policy from requesting another turn.

### Change

Use `before_finalize` for behavior-changing terminal review. Post-terminal side effects are observer responsibilities.

```text
candidate result -> before_finalize -> terminal record
```

### Why it matters

Verification loops, human review before acceptance, structured policy checks, and evidence requirements all depend on a clear pre-terminal boundary.

## 17.5 No additional primary port

The review explicitly rejects adding:

- `Subagent`;
- `Memory`;
- `RAG`;
- `Workflow`;
- `Human`;
- `Scheduler`;
- `Channel`;
- `Guardrail`; or
- `Sandbox`

as primary kernel ports.

Each can be composed from current ports plus runtime/application services.

# 18. Proposed type and record changes

## 18.1 Run relation

```rust
pub struct RunRelation {
    pub root_run_id: RunId,
    pub parent_run_id: Option<RunId>,
    pub parent_effect_id: Option<EffectId>,
    pub kind: RunRelationKind,
    pub depth: u16,
    pub budget_scope_id: Option<BudgetScopeId>,
    pub external_work_ref: Option<Arc<str>>,
}

pub enum RunRelationKind {
    Root,
    ChildAgent,
    DelegatedAgent,
    WorkflowStep,
}
```

`RunAccepted` stores the relation. `Root` is explicit to simplify trace processing.

## 18.2 Deferred effect

```rust
pub struct EffectDeferred {
    pub effect_id: EffectId,
    pub handle: ExternalHandleRef,
    pub reconciliation: ReconciliationPolicy,
    pub next_poll_at: Option<Timestamp>,
    pub expires_at: Option<Timestamp>,
    pub expected_output_schema: Option<Digest>,
}

pub enum ReconciliationPolicy {
    CallbackOnly,
    Poll,
    CallbackOrPoll,
    ExternalWorkflow,
}
```

The public handle may contain a provider/job identifier but never a bearer secret. Host-private credentials are keyed by `EffectId`.

External completion enters through a runtime command:

```rust
pub struct ExternalEffectCompletion {
    pub effect_id: EffectId,
    pub completion_id: Arc<str>,
    pub output: RawJson,
    pub usage: Option<Usage>,
    pub artifacts: Arc<[ArtifactRef]>,
}
```

Duplicate `completion_id` or identical output digest is idempotent. A conflicting duplicate is an audit error.

## 18.3 Interaction

```rust
pub struct InteractionRequest {
    pub interaction_id: InteractionId,
    pub kind: InteractionKind,
    pub prompt: Arc<[ContentBlock]>,
    pub response_schema: RawJson,
    pub assignee_hint: Option<AssigneeHint>,
    pub expires_at: Option<Timestamp>,
    pub delegatable: bool,
    pub metadata: RawJson,
}

pub enum InteractionKind {
    Approval,
    Choice,
    Form,
    FreeText,
    Review,
    Custom(Arc<str>),
}

pub struct InteractionResolution {
    pub interaction_id: InteractionId,
    pub status: InteractionResolutionStatus,
    pub principal: PrincipalRef,
    pub response: Option<RawJson>,
    pub comment: Option<Arc<str>>,
}
```

The runtime validates the response against the recorded schema before committing resolution.

## 18.4 Record changes

Recommended `RecordBody` updates:

```text
EffectDeferred
InteractionRequested
InteractionResolved
InteractionExpired
InteractionCancelled
```

Approval-specific public records become compatibility aliases or pre-1.0 replacements. `RunSuspended` retains a normalized reason referencing the outstanding effect or interaction.

## 18.5 Runtime additions outside the kernel

```text
AgentCatalog
AgentInvoker
ExternalCompletionRouter
InteractionRouter
BudgetLedger
ArtifactStore service
BundleResolver
```

These are reusable services, not kernel ports.

# 19. Implementation-plan impact

The current 66-PR implementation plan remains valid. The following amendments minimize rework.

| Existing PR | Recommended amendment |
|---|---|
| PR-008 | Include `EffectDeferred`, generalized interaction records, run lineage fields, and fixtures |
| PR-009/010 | Ensure reducer can enter and leave `AwaitingExternal` without feature-specific branches |
| PR-011 | Include cancellation/deadline behavior for deferred effects and interactions |
| PR-014 | Add runtime command routing for external completion and deterministic duplicate handling |
| PR-018 | Define `before_finalize`; ensure middleware can request continuation before terminal commit |
| PR-021/022 | Add `AgentCatalog`, `AgentInvoker` service wiring, and non-kernel `BundleSpec` |
| PR-023 | Add conformance helpers for child runs, deferred completion, and interaction schemas |
| PR-030/034 | Bind external completion, child-run handles, and interaction APIs coarsely |
| PR-039 | Freeze the generalized records rather than approval-only records |
| PR-042 | Use generic deferred-effect reconciliation for background model responses |
| PR-043 | Use the same mechanism for external/long-running tool completion |
| PR-044 | Rename and implement durable interactions; ship approval as the first profile |
| PR-046/047 | Persist and expose run lineage; add child-run/lane stress tests |
| PR-049/050 | Keep WIT v1 limited to toolset/context; do not expose full agent invocation initially |
| PR-056 | Include first-party reference memory/RAG and verification capability patterns or examples |
| PR-059 | Map workflow waits and callbacks to interactions/deferred effects rather than custom semantics |

## 19.1 Suggested post-preview battery sequence

After the core public preview, implement higher-level batteries in this order:

1. `finstack-ai-memory` reference capability with in-memory/SQLite test store.
2. `finstack-ai-knowledge` retrieval interfaces and one reference search/index adapter.
3. `finstack-ai-agents` with `AgentCatalog`, `AgentToolset`, child-run policies, and fan-out helpers.
4. `finstack-ai-interactions` UI-neutral request router plus CLI/web examples.
5. `finstack-ai-workflow` examples/adapters for one durable workflow system.
6. `finstack-ai-sandbox` reference process or WASI code-execution toolset.
7. Channel/server examples mapping external thread identities to lanes.
8. `finstack-ai-evals` trace fixtures, offline scorers, and verification middleware examples.

These packages prove the architecture without broadening the kernel.

# 20. Conformance and future-proofing tests

## 20.1 Cross-binding semantic fixtures

Rust, Python, and browser WASM should produce equivalent durable records and public events for:

1. parent run invoking one child agent;
2. parallel child runs completing out of order but aggregating deterministically;
3. parent cancellation cascading to children;
4. deferred tool completion after process restart;
5. duplicate external completion;
6. approval interaction;
7. typed form interaction;
8. interaction expiry and cancellation;
9. memory recall contribution with deterministic ordering;
10. RAG results with provenance and blob references;
11. `before_finalize` rejecting a candidate result and causing another turn;
12. channel/client disconnect while a durable run continues; and
13. long-running sandbox job using callback or polling reconciliation.

## 20.2 Crash-prefix tests

Crash tests should be generated before and after:

- parent tool effect commit;
- child `RunAccepted` commit;
- child terminal commit;
- parent tool completion commit;
- `EffectDeferred` commit;
- external completion receipt;
- interaction request commit;
- interaction resolution receipt; and
- pre-finalize middleware outcome commit.

Every prefix must restore to completed, retryable, suspended, cancelled, failed, or explicit uncertain state.

## 20.3 Property tests

Useful properties include:

- one child `RunId` per parent `EffectId` under deterministic invocation;
- no outstanding tool call receives more than one conflicting result;
- a deferred effect cannot complete after terminal cancellation unless policy explicitly accepts late results;
- an interaction resolution must match its recorded schema;
- run lineage is acyclic;
- depth and root IDs remain consistent;
- capability activation occurs only at safe checkpoints; and
- context-provider order is deterministic regardless of completion order.

## 20.4 Performance tests

Measure:

- direct native child invocation overhead;
- 100 parallel child runs with scripted models;
- memory/RAG provider fan-out and budget assembly;
- external completion resume latency;
- Python interaction and child-run handle overhead;
- WASM event batching for child streams; and
- journal replay with deep run lineage.

The benchmark should isolate framework overhead from network/model latency.

# 21. Risks and architectural anti-patterns

## 21.1 Turning the kernel into a workflow engine

Adding generic nodes, edges, conditions, compensation, scheduling, or business variables to the kernel would expand its responsibility beyond one agent run. Keep workflows above the run API.

## 21.2 Treating every feature as a new port

Ports are stable implementation families, not feature labels. Memory, RAG, channels, and subagents compose existing ports.

## 21.3 Hiding nested runs inside opaque tools

A tool may invoke a child agent, but durable implementations must record explicit run lineage. Otherwise cancellation, usage, audit, and recovery become unreliable.

## 21.4 Allowing arbitrary middleware mutation

Capabilities should return normalized outcomes. They must not mutate session/kernel state directly or retain mutable references across awaits.

## 21.5 Synchronous re-entrant lane execution

A parent tool must not synchronously acquire a child operation on the same busy lane. Child runs use a new lane/session and are scheduled by `AgentInvoker` to avoid deadlocks.

## 21.6 Copying full child transcripts into parent context

Return summaries, structured outputs, and artifacts. Full transcripts remain inspectable but opt-in.

## 21.7 Using observers for required side effects

Memory writes, audit-required approvals, and business actions that must affect correctness cannot rely solely on best-effort observers. Use tools, middleware effects, or external workflows with stable IDs.

## 21.8 Making WIT the native hot path

Trusted native extensions use direct Rust traits. WIT/process boundaries are for isolation and independent distribution.

## 21.9 Storing external secrets in journal handles

Journal records contain opaque identifiers and non-secret reconciliation metadata. Credentials remain in host secret stores.

## 21.10 Feature bundles that hide incompatible policies

Bundle resolution must surface conflicts in tool permissions, memory scope, middleware order, store ownership, and interaction routing before agent construction.

# 22. Final decision and acceptance checklist

## 22.1 Final decision

The current microkernel architecture is fit for the future capability set reviewed here. The framework should proceed without adding new primary ports or a general workflow graph.

Before public journal/API contracts freeze, implement or specify:

- generic deferred effects;
- run lineage;
- generalized interactions; and
- a pre-terminal `before_finalize` middleware stage.

Add `AgentCatalog`, `AgentInvoker`, `BundleSpec`, interaction routing, external completion routing, budget aggregation, and artifact services in the SDK/runtime/application layers.

## 22.2 Architecture acceptance checklist

The design is ready for these future capabilities when all answers below are yes.

| Question | Required answer |
|---|---|
| Can a child run be correlated to its parent across restart? | Yes |
| Can a model/tool job complete after the originating process exits? | Yes |
| Can an interaction collect typed input beyond approve/deny? | Yes |
| Can memory and RAG contribute context without mutating history? | Yes |
| Can a verification policy prevent terminal completion? | Yes |
| Can workflows compose runs without replacing run semantics? | Yes |
| Can native components stay direct-call and registry-free in the hot path? | Yes |
| Can Python/WASM expose the same behavior through coarse handles/events? | Yes |
| Can untrusted tools/context move behind WIT/process isolation? | Yes |
| Can all ten capabilities be implemented without a new kernel port? | Yes |

# Appendix A. Feature-to-primitive traceability

| Feature | Kernel primitives | Ports | Runtime/application services |
|---|---|---|---|
| Subagents | Runs, effects, lanes, lineage, events | Model, Toolset, Store, Observer | AgentCatalog, AgentInvoker, budget ledger |
| Delegation | Effects, deferral, suspension, interactions | Toolset, Middleware, Store | Work-item service, completion router |
| Memory | Sessions/messages, capability activation | ContextProvider, Toolset, Middleware, Observer | Memory store/index |
| RAG | Context budgets, blobs, events | ContextProvider, Toolset | Ingestion/indexing service |
| HITL workflow | Interactions, timers, suspension, journal | Middleware, Toolset, Store | Workflow engine, inbox/router |
| Background work | Runs, timers, deferral, cancellation | Model, Toolset, Store | Scheduler/server/workers |
| Multi-agent teams | Runs, lanes, lineage, effects | Model, Toolset, Observer | Fan-out coordinator, AgentInvoker |
| Sandbox/computer use | Tool effects, blobs, progress, cancellation | Toolset | Process/WASI/remote sandbox host |
| Multi-channel | Sessions, lanes, principals, events | Context/tool/middleware as needed | Channel adapters, session server |
| Guardrails/evals | Events, middleware checkpoints, terminal transition | Middleware, Observer, Toolset | Eval runners, policy/config services |

# Appendix B. Example bundle specifications

## B.1 Research bundle

```json
{
  "schema_version": 1,
  "id": "finstack.research",
  "version": "0.1.0",
  "agents": ["coordinator", "researcher", "verifier"],
  "capabilities": [
    "knowledge.auto_context",
    "knowledge.search",
    "memory.read",
    "subagents.research",
    "verification.citations"
  ],
  "required_components": [
    "model.primary",
    "model.research",
    "context.knowledge",
    "toolset.knowledge",
    "toolset.agent_invocation",
    "middleware.citation_verification",
    "store.journal"
  ]
}
```

## B.2 Human-reviewed action bundle

```json
{
  "schema_version": 1,
  "id": "finstack.reviewed-action",
  "version": "0.1.0",
  "agents": ["operator"],
  "capabilities": [
    "enterprise.context",
    "interaction.review",
    "interaction.action_approval",
    "audit.events"
  ],
  "required_components": [
    "middleware.review",
    "middleware.approval",
    "toolset.enterprise_actions",
    "observer.audit",
    "store.journal"
  ]
}
```

## B.3 Coding bundle

```json
{
  "schema_version": 1,
  "id": "finstack.coding",
  "version": "0.1.0",
  "agents": ["coder", "reviewer"],
  "capabilities": [
    "repository.context",
    "filesystem",
    "sandbox.shell",
    "subagents.review",
    "verification.tests"
  ],
  "required_components": [
    "context.repository",
    "toolset.filesystem",
    "toolset.sandbox",
    "toolset.agent_invocation",
    "middleware.test_verification"
  ]
}
```

# Appendix C. Glossary

**AgentCatalog:** Runtime/SDK registry of pre-resolved agents that may be invoked as children.

**AgentInvoker:** Runtime service that starts and monitors child runs using the normal kernel.

**Artifact:** Large or versioned work product referenced by metadata and a `BlobRef`.

**Bundle:** Deployment/application composition of agents, capabilities, required components, and defaults.

**Capability:** Declarative behavior assembled from instructions and references to registered toolsets, context providers, and middleware.

**Deferred effect:** Requested external work that has not produced a final result but has a durable handle and future completion path.

**Delegated work:** Work assigned to another agent, person, service, or worker, either synchronously or asynchronously.

**Interaction:** Durable request for typed external input, including approval, questions, forms, choices, or review.

**Run lineage:** Root/parent relationship metadata connecting nested or delegated runs.

**Subagent:** Another resolved agent invoked by a parent run for bounded work.
