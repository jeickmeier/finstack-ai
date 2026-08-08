---
title: "finstack-ai Security and Threat Model"
subtitle: "Assets, trust boundaries, threats, controls, and verification obligations"
author: "finstack-ai project"
date: "2026-08-08"
---

# finstack-ai Security and Threat Model

# Document control

| Field | Value |
|---|---|
| Product | finstack-ai |
| Document | Security and Threat Model |
| Version | 0.4 |
| Status | Pre-implementation security baseline |
| Date | 2026-08-08 |
| Primary audience | Maintainers, security reviewers, runtime/binding/plugin implementers, deployers, and extension authors |
| Related documents | Product Requirements Document v0.7; Architecture Specification v0.6; Technical Design v0.8; Implementation Plan v0.8; Engineering Standards v0.4 |

# 1. Purpose and authority

This document defines the security assumptions, assets, trust boundaries, threats, required controls, and verification obligations for `finstack-ai`. It owns security analysis and control traceability. The PRD owns security outcomes, the Architecture Specification owns system/trust boundaries, the Technical Design owns implementation mechanisms, and accepted ADRs own changes to those decisions.

This is a design-time threat model, not a claim that an implementation has passed security review. Each delivery gate must replace planned controls with test, audit, or operational evidence. A security-sensitive design change must update this document in the same review unit.

# 2. Scope and security objectives

## 2.1 In scope

- the deterministic kernel and native runtime;
- native Rust extensions and batteries;
- Python callbacks and Rust-backed Python execution;
- browser WASM and JavaScript host adapters;
- durable journals, snapshots, artifacts, and external completion;
- generalized human interactions;
- remote client/session protocols and reference server;
- WIT components, the optional Wasmtime host, and process-isolated adapters;
- providers, toolsets, context providers, middleware, observers, workflow adapters, and bundles; and
- build, dependency, packaging, signing, and release paths.

## 2.2 Security objectives

1. Preserve the integrity of kernel state, records, lineage, effect identity, and terminal outcomes.
2. Prevent unauthorized tools, effects, completions, interaction resolutions, and resource access.
3. Keep credentials and protected content out of ordinary prompts, journals, events, errors, and telemetry by default.
4. Bound damage from untrusted model output, retrieved content, schemas, protocol input, and isolated extensions.
5. Make policy decisions, permission grants, denials, uncertain outcomes, and privileged actions auditable.
6. Preserve availability through explicit bounds, deadlines, cancellation, backpressure, and resource limits.
7. Make trust differences between native, host-language, remote, and isolated extension paths explicit.
8. Ship verifiable artifacts with known source, dependency, and compatibility provenance.

## 2.3 Non-goals and deployment responsibilities

`finstack-ai` does not claim to:

- sandbox arbitrary native Rust, Python, or JavaScript code in-process;
- make model output or retrieved content trustworthy;
- guarantee exactly-once behavior in external systems;
- provide a universal identity provider, secrets manager, key-management system, DLP service, malware scanner, or network firewall;
- encrypt every store or artifact backend independently of the host platform;
- prevent a fully privileged host application from reading data it owns;
- make a shell, browser/computer-use tool, or filesystem safe without restrictive application policy; or
- turn WASM isolation alone into application-level authorization.

Deployers own tenant isolation, identity proofing, credential issuance/rotation, host hardening, network policy, store encryption, backup access, retention/deletion policy, and legal/privacy compliance. Reference components must expose the hooks and safe defaults needed to meet those responsibilities.

# 3. Assets and data classification

| Class | Examples | Default handling |
|---|---|---|
| Public | Published schemas, package metadata, public docs, non-sensitive model identifiers | May be logged and cached subject to integrity checks. |
| Internal | Run IDs, timing, usage, non-secret configuration, tool names, diagnostic metadata | Authenticated access; metadata-only telemetry preferred. |
| Sensitive | Prompts, model responses, retrieved documents, interaction responses, tool arguments/results, conversation history, artifacts | Least privilege, redaction, bounded retention, authenticated access, encryption by host/store policy. |
| Secret | API keys, OAuth tokens, cookies, private keys, resume credentials, signing keys | Scoped references/handles only; never ordinary prompt/event/journal/bundle content. |
| Security control data | Permission grants, principal/tenant scope, effect/interaction authorization, signature status, audit records | Integrity protected, durable where behavior depends on it, tightly writable, auditable. |

Critical integrity assets are the journal sequence, record meaning, state version, `RunId`/`EffectId`/`InteractionId` relations, policy outcomes, external completion identity/digest, plugin identity/digest, and release provenance.

# 4. Trust model and boundaries

```text
untrusted or semi-trusted inputs
  users / models / retrieved content / remote clients / plugin packages
                              |
                              v
application authentication, authorization, policy, and validation
                              |
              +---------------+----------------+
              |                                |
              v                                v
       native runtime                    isolated hosts
   trusted application process       WIT/Wasmtime or process
              |                                |
              +---------------+----------------+
                              |
                              v
                 deterministic semantic kernel
                    no I/O or ambient authority
                              |
                              v
             versioned journal and public events
```

## 4.1 Trust classes

| Class | Components | Security meaning |
|---|---|---|
| T0 Semantic core | Kernel types, reducer, record application | Trusted for semantic integrity; must remain deterministic and I/O-free. |
| T1 Host/runtime | Native runtime, SDK, application, native providers/tools/stores | Fully trusted within the process and OS account; bugs or malicious code can compromise the application. |
| T2 Host-language callbacks | Python and JavaScript callbacks/adapters | Trusted application code with language-runtime reentrancy, lifecycle, and secret-exposure risks; not a sandbox. |
| T3 Isolated extension | WIT/Wasmtime component or external process | Untrusted by default; receives only linked capabilities, bounded inputs, and explicit resources. |
| T4 Remote principal | Remote client, external completion caller, interaction resolver, workflow callback | Untrusted until authenticated and authorized for a precisely scoped action. |
| T5 Content | User/model/retrieved documents, tool data, schemas, artifacts | Data, never authority. May contain adversarial instructions or malformed structures. |

## 4.2 Boundary rules

- Crossing from T4/T5 into runtime behavior requires authentication where applicable, size/schema validation, policy evaluation, and stable identity correlation.
- Crossing from T3 into host resources requires an explicitly linked capability and per-call limits.
- Crossing from T1/T2 into T0 uses normalized commands only. The kernel never calls host code.
- Durable records contain semantic facts and non-secret references, not live credentials or host object handles.
- Observers receive a redacted view appropriate to their trust level and cannot affect semantic execution.

# 5. Threat actors and capabilities

The model considers:

- an unauthenticated network client sending arbitrary, fragmented, oversized, or replayed frames;
- an authenticated user attempting cross-tenant access or privilege escalation;
- a malicious or compromised model provider returning adversarial text, tool calls, usage, or stream frames;
- poisoned retrieved content attempting prompt injection or unauthorized tool use;
- a malicious tool, Python/JavaScript callback, native dependency, plugin component, or plugin package;
- a user resolving or delegating an interaction they are not authorized to control;
- an attacker replaying or conflicting an external effect completion;
- a local attacker with access to journal, snapshot, artifact, cache, or log files;
- a compromised dependency, build action, package registry artifact, release credential, or signing key; and
- accidental misuse, misconfiguration, cancellation races, partial failure, or implementation defects.

A malicious host application, OS administrator, or fully trusted native extension is outside the isolation guarantee. The framework must document that authority rather than imply protection it cannot provide.

# 6. Security invariants

| ID | Required invariant |
|---|---|
| SEC-INV-001 | Data content never grants authority by itself; model/user/retrieved instructions cannot bypass application tool and resource policy. |
| SEC-INV-002 | No external effect executes before committed durable intent when recovery is enabled. |
| SEC-INV-003 | External completion and interaction resolution are authenticated, authorized, schema-validated, and correlated to one outstanding identifier. |
| SEC-INV-004 | Equivalent duplicates are idempotent; conflicting or late privileged inputs fail closed and are audited. |
| SEC-INV-005 | Secret values are not stored in ordinary records/events, sent to models, or exported to observers by default. |
| SEC-INV-006 | Isolated extensions have no ambient filesystem, network, environment, process, or secret access. |
| SEC-INV-007 | All untrusted lengths, counts, nesting, concurrency, time, retries, fuel, and retained output are bounded before resource commitment. |
| SEC-INV-008 | Principal, tenant/scope, run lineage, effect identity, and plugin identity cannot be silently substituted across a privileged action. |
| SEC-INV-009 | Terminal records are immutable; post-terminal observers cannot change execution. |
| SEC-INV-010 | Journal corruption, version mismatch, or impossible state produces an explicit error rather than guessed state. |
| SEC-INV-011 | Permission grants, policy outcomes, plugin identity/digest, and privileged actions are observable without exposing their secrets. |
| SEC-INV-012 | Minimal/default builds do not silently include privileged batteries, providers, telemetry exporters, plugin engines, or network services. |
| SEC-INV-013 | Compaction preserves mandatory policy/safety content, provenance, and sensitivity; derived summaries/checkpoints never replace or weaken canonical audit history. |

# 7. Threat and control register

| ID | Threat | Required controls | Primary verification and delivery |
|---|---|---|---|
| TM-01 | Prompt injection or poisoned retrieval causes unauthorized action. | Treat content as data; provenance; explicit tool/resource policy; schema validation; optional interaction/verification before sensitive effects. | Adversarial context/tool-policy fixtures; PR-016, PR-018, PR-056. |
| TM-02 | Model fabricates, mutates, or floods tool calls. | Validate names/schemas; enforce call, size, cost, and concurrency limits; reject unknown tools; preserve source ordering; policy before dispatch. | Tool scheduler and malformed-call tests; PR-010, PR-015, PR-018. |
| TM-03 | Filesystem or shell tool escapes intended scope. | Handle-relative no-follow/capability-style filesystem access; symlink/rename race tests; allow/deny command policy; minimal environment; bounded output/time; optional external sandbox. | Traversal, symlink, environment, injection, timeout, and flood tests; PR-025, PR-056. |
| TM-04 | Secrets leak through prompts, errors, journals, events, artifacts, or telemetry. | Secret references; explicit reveal boundaries; structured redaction and metadata-only observer/diagnostic/export views; secure references in authoritative records; deny secret fields in bundles; safe error display. | Canary-secret and redaction tests; PR-003, PR-014, PR-022, PR-024, PR-034, PR-039, PR-057, PR-060. |
| TM-05 | Browser bundle exposes provider credentials or sensitive persistent data. | Same-origin proxy pattern; no embedded provider key; documented CSP/CORS/storage policy; explicit IndexedDB sensitivity and deletion behavior. | Static bundle/example scanning and browser security fixtures; PR-034, PR-037, PR-038. |
| TM-06 | Malicious native/Python/JS extension is mistaken for isolated code. | Explicit trust labels; opt-in registration; documentation that in-process extensions inherit host authority; isolate untrusted code through T3 path. | API docs, starter review, trust-boundary tests; PR-021, PR-030, PR-034, PR-060. |
| TM-07 | WIT/plugin escapes sandbox or exhausts host. | Deny-by-default WASI; explicit linked capabilities; no ambient preopens/network; fuel/epoch deadlines; memory/table/instance limits; trap containment. | Hostile component suite; PR-049 through PR-054. |
| TM-08 | Plugin package substitution or incompatible ABI. | Digest/signature verification; configured trust roots; lockfile resolution; interface version negotiation; duplicate identity rejection; compiled-cache key includes digest/engine/target/ABI. | Tampered package, signature, downgrade, duplicate, and cache tests; PR-050 through PR-054. |
| TM-09 | Remote frame causes parsing abuse, downgrade, or session confusion. | Authenticate before trusting protocol bytes; length check before allocation; strict CBOR profile; version handshake; payload-family separation; correlation IDs; parser fuzzing. | Protocol fuzz/size/version/auth fixtures; PR-058. |
| TM-10 | Spoofed or replayed external completion changes a run. | Authenticated router; effect/session/tenant scope; original `EffectId`; completion identity and normalized outcome digest; expiry/cancellation check; idempotent duplicate and fail-closed conflict. | Cross-binding duplicate/conflict/restart tests; PR-014, PR-023, PR-039, PR-042/043, PR-048. |
| TM-11 | Unauthorized or replayed interaction resolution approves an action. | Assignee/principal authorization; interaction/effect/session scope; response schema; resolution identity/digest; expiry/cancellation check; immutable audit. | Approval, form, delegation, expiry, late/conflict fixtures; PR-044, PR-048. |
| TM-12 | Journal tampering, truncation, reordering, or incompatible decoding corrupts state. | Atomic append; sequence/version checks; deterministic encoding; checksums; strict bounds; corruption classification; authoritative journal; migration fixtures; protected backend. | Crash-prefix, corruption, historical-fixture, and backup/restore tests; PR-039 through PR-041, PR-048. |
| TM-13 | Snapshot injects stale or forged state. | Versioned state-CBOR; journal position and state hash; validate before use; discard/rebuild on mismatch; snapshots never authoritative. | Mutation, mismatch, deletion, and replay-equivalence tests; PR-041. |
| TM-14 | Cancellation/deadline race permits a late privileged effect or completion. | Durable cancellation intent; effect state check at dispatch and settlement; idempotent cleanup; explicit late-result policy; framework-authored cancelled closures carry no tool output/success claim and fabricated successful/tool-produced results are prohibited. | Boundary fault injection and late-result matrix; PR-011, PR-015, PR-045, PR-048. |
| TM-15 | Child runs evade principal, budget, depth, deadline, or cancellation policy. | Immutable run relation; maximum depth; explicit propagation policy; budget scope; child invocation owned by authenticated runtime/SDK service. | Lineage property tests and restart/concurrency stress; PR-008, PR-011, PR-022, PR-046/047. |
| TM-16 | Oversized or malicious schema/JSON/blob causes memory or parser denial of service. | Pre-allocation byte/depth/count limits; bounded validation; digest/reference large blobs; no automatic recursive remote fetch; streaming limits. | Fuzzing and size/depth boundary tests; PR-006 through PR-008, PR-013, PR-020, PR-039, PR-058. |
| TM-17 | Observer/exporter failure, blockage, or payload leak changes behavior. | Observers are non-semantic; bounded exporter queues; redacted event views; drop/disconnect policy for progress; protect durable completion. | Slow/failing observer and redaction tests; PR-017, PR-018, PR-057. |
| TM-18 | Dependency or release compromise ships malicious code. | Reviewed dependencies; source/license/advisory policy; immutable CI actions/tools; least-privilege release credentials; SBOM, checksums, signatures/provenance, reproducible release. | Supply-chain CI and release rehearsal; PR-003, PR-060, PR-061, PR-065/066. |
| TM-19 | Tenant/session identifiers are swapped at an application or workflow boundary. | Bind authenticated principal and tenant scope to acquired session/run handles; authorize every resume/inspect/cancel/resolve action; avoid user-selected ownership fields. | Cross-tenant negative tests in server/workflow adapters; PR-045, PR-058, PR-059. |
| TM-20 | Artifact/blob reference grants unintended data access. | Opaque scoped references; authorization on dereference; content type/size/digest metadata; separate read/write grants; malware/content scanning hook for deployments that require it. | Reference-confusion, cross-scope, digest, and size tests in artifact adapters; PR-022, PR-037, PR-056, PR-058. |
| TM-21 | Compaction removes safety/policy context, distorts provenance, or leaks sensitive history through summaries/checkpoints. | Unique late-tier `before_model` middleware ownership; protected/non-compactable item set; tool-pair atomicity; inherited sensitivity/provenance; version/config/model-profile/source/protected-set/projection digests; canonical history immutability; redacted diagnostics; safe failure when budget cannot be met. | Adversarial compaction, checkpoint invalidation, secret-canary, replay, and cross-binding tests; PR-018, PR-023, PR-048, PR-056/057. |

# 8. Surface-specific control requirements

## 8.1 Kernel and native runtime

- The kernel accepts normalized inputs and has no ambient I/O, secrets, callbacks, or dynamic loading.
- State transitions reject identifier, phase, sequence, lineage, and outstanding-effect mismatches.
- Runtime dispatch rechecks cancellation/deadline and committed effect state immediately before privileged external execution.
- Native extension registration is explicit. Component identity and trust classification are visible in diagnostics.
- No public API may imply that a trusted native tool is sandboxed.

## 8.2 Python

- Python callbacks are trusted and may access process memory; documentation and APIs must state this.
- Callback entry has explicit lifetime, cancellation, timeout, reentrancy, and exception normalization.
- The Rust runtime must not hold locks across Python calls.
- Tracebacks exported outside the process are sanitized according to application policy.
- Free-threaded builds must pass concurrent handle, callback, shutdown, and object-lifetime tests.

## 8.3 Browser and JavaScript

- Browser adapters receive only host-provided capabilities and URLs.
- The reference remote-model adapter defaults to an application-controlled endpoint and contains no credential persistence helper that encourages browser provider keys.
- Worker messages and transferred buffers are size-bounded and correlated to owned handles.
- IndexedDB stores are treated as sensitive local data; examples document origin access, logout/deletion, schema upgrade, and shared-device risk.
- Cross-origin isolation or shared-memory requirements cannot become an undeclared default.

## 8.4 WIT/Wasmtime and process isolation

- The host constructs a fresh deny-by-default resource context from validated manifest and application grants.
- Guest-supplied paths, URLs, environment keys, and capability names are untrusted.
- Host functions authorize the concrete resource operation, not only the plugin at load time.
- Cancellation, timeout, trap, and host shutdown release guest resources predictably.
- Progress is advisory; final completion/error is the v1 semantic boundary.
- External process adapters reuse hardened framing but have a vocabulary distinct from remote session control.

## 8.5 Remote server, completions, and interactions

- The framework defines authentication hooks and scoped authorization inputs; applications select the identity mechanism.
- Authentication occurs before session acquisition or privileged payload processing.
- A principal must be authorized for the exact session/run/effect/interaction action; possession of an ID alone is insufficient.
- Replay protection is based on stable command/completion/resolution identity plus normalized payload digest and retained settlement state.
- Error responses do not reveal whether unauthorized identifiers exist.
- Rate, connection, frame, session, and outstanding-operation limits are configurable and observable.

## 8.6 Journals, snapshots, and artifacts

- Store backends protect confidentiality and integrity according to deployment risk; local defaults document file permissions and encryption limitations.
- Secrets are represented by non-secret references. A resume credential is never a journal handle.
- Backups and diagnostic exports inherit the source data classification and redaction policy.
- Snapshot or cache deletion cannot remove the authoritative audit trail required for recovery.
- Pruning/retention requires explicit policy, compatibility-safe checkpoints, and application-level legal approval where applicable.

# 9. Authentication and authorization model

The framework remains identity-provider neutral. It standardizes the authorization context needed to make consistent decisions:

```text
principal reference
tenant/security scope
authentication strength/method metadata
session and lane ownership
run relation and initiating principal
target effect/interaction/plugin/artifact
requested action
policy version and decision identity
```

Rules:

- principal and tenant references come from the authenticated host, not model output or request payload ownership fields;
- authorization is evaluated at handle acquisition and again for privileged state-changing actions;
- delegated interaction or child-run authority is explicit, scoped, expiring where practical, and auditable;
- provider/tool/plugin identity is included in policy input when behavior depends on it;
- policy denial is a normal, stable outcome and must not be converted into a retry that bypasses policy; and
- cached authorization decisions include policy version, relevant scope, and a safe invalidation rule.

# 10. Secrets and credential handling

- Configuration schemas and bundles may declare secret references, never secret values.
- The host resolves a secret as late as practical and passes it only to the adapter that needs it.
- Adapters must not add credentials to debug representations, URLs, tracing fields, panic messages, or persisted retry metadata.
- Redirects, proxies, provider base URLs, and plugin egress can change where credentials are sent; adapters validate or delegate these decisions to explicit host policy.
- Rotation must not require rewriting historical records. Reconciliation stores only a non-secret external handle; privileged resume credentials remain in a secret service.
- Tests use canary secrets to prove redaction across errors, journal projections, event exporters, Python exceptions, browser bundles, and recovery diagnostics.

# 11. Logging, observability, privacy, and retention

## 11.1 Event views

Observers select an application-approved view:

- metadata only;
- redacted structured content;
- full sensitive content for a tightly controlled local sink; or
- custom field policy.

Full content is never the safe default for remote telemetry. Redaction happens before data enters an exporter queue.

## 11.2 Correlation without disclosure

Logs and traces may include stable run/effect/interaction/plugin IDs, state phase, duration, sizes, usage, and stable error codes. They should not include raw prompts, schemas, tool arguments/results, interaction responses, document text, tokens, credentials, or filesystem contents unless an explicit policy enables them.

## 11.3 Retention and deletion

The framework exposes deletion/pruning and export hooks where supported, but applications own retention periods and legal policy. Deleting conversation content does not silently falsify required audit or recovery records; designs must define tombstone, redaction, storage compaction, or retention behavior before claiming deletion support. Model-context compaction is a separate `before_model` middleware concern and is never a deletion mechanism.

# 12. Supply-chain and release security

- Repository and release permissions follow least privilege and separation between ordinary CI and release publication.
- External CI actions, build images, toolchains, generators, and binary tools are pinned to immutable versions/digests where supported.
- Dependency sources, advisories, licenses, and exceptions are checked continuously.
- Release artifacts are produced from tagged reviewed source and include SBOMs, checksums, provenance, and signatures where the ecosystem supports them.
- Python wheels, npm packages, crates, WIT packages, and server binaries share one release manifest and semantic engine version mapping.
- Signing keys and registry tokens are short-lived or hardware/managed-secret backed where practical and are unavailable to untrusted pull-request jobs.
- A release rehearsal verifies artifact contents, feature sets, generated files, package ownership, and reproducibility before public preview and GA.

# 13. Security verification program

## 13.1 Continuous checks

- dependency advisory/license/source policy;
- secret scanning and canary-redaction tests;
- static linting and unsafe-code review enforcement;
- minimal/default/all-feature dependency and target builds;
- parser and deserializer fuzz targets;
- authorization negative tests and cross-scope identifier tests;
- queue/payload/depth/time/concurrency boundary tests; and
- documentation checks for trust labels and insecure example patterns.

## 13.2 Gate evidence

| Gate | Required security evidence |
|---|---|
| G0 Foundation | Threat model accepted; security owner; reporting process; dependency/secret policy; architecture checks designed. |
| G1 Kernel Semantics | Transition, malformed-record, limit, lineage, interaction, idempotency, and fuzz evidence. |
| G2 Native Runtime | Commit-before-effect fault tests; cancellation races; bounded queues; safe shutdown; redacted errors/events. |
| G3 Native Preview | Provider/tool threat review; filesystem policy tests; credential and endpoint guidance. |
| G4 Binding Parity | Python/JS trust documentation; callback lifecycle tests; browser bundle secret scan; cross-binding security traces. |
| G5 Durable Beta | Corruption, replay, duplicate completion/resolution, backup/restore, migration, and cross-scope recovery tests. |
| G6 Plugin Alpha | Deny-by-default permissions; hostile component tests; limits; signature/lockfile/downgrade evidence. |
| G7 Public Preview | Updated threat model; external surface review; SBOM/provenance; vulnerability process; security deployment guide. |
| G8 1.0 GA | Independent review closed; no unaccepted critical/high findings; fuzz/recovery soak; release credential/reproducibility rehearsal. |

## 13.3 Independent review scope

Before 1.0, an independent review must cover at least:

- journal/recovery integrity and external completion;
- remote framing, authentication hooks, and authorization context;
- interaction authorization and privileged tool dispatch;
- filesystem/shell and artifact boundaries;
- WIT/Wasmtime permissions, limits, signatures, and cache identity;
- Python/JavaScript callback lifecycle and browser credential guidance;
- secret/redaction behavior; and
- dependency/build/release provenance.

# 14. Vulnerability and incident response requirements

Phase 0 establishes a private reporting path, named response owner, severity rubric, acknowledgement target, embargo process, and supported-version policy. Before public preview:

- `SECURITY.md` identifies supported versions and reporting instructions;
- maintainers can revoke releases, plugin trust roots, and compromised publishing credentials;
- the release process can issue advisories and patched artifacts across crates, PyPI, npm, WIT, and binaries;
- logs contain sufficient stable identifiers to investigate without requiring sensitive payload collection; and
- incident handling documents when users must rotate secrets, rebuild journals/caches, or invalidate artifacts/plugins.

Security fixes that alter durable meaning or public protocols still require compatibility handling. Embargoed details may be reconciled into public ADRs and this threat model when disclosure is safe.

# 15. Deployment configuration gates

The following choices are required before deploying the affected surface but do not block kernel implementation:

| Choice | Owner and decision point |
|---|---|
| Concrete remote authentication mechanism | Reference server/application owner before PR-058 is production-enabled. |
| Tenant authorization policy and identity mapping | Deploying application before multi-user sessions. |
| Journal/artifact encryption backend and key rotation | Store/deployment owner before sensitive durable use. |
| Data retention, deletion, export, and legal hold | Product/deployment owner before storing user data. |
| Plugin publisher trust roots and signature strictness | Host administrator before third-party plugin enablement. |
| Network egress policy for native tools/providers and isolated capabilities | Deployment owner before privileged network access. |
| Sandbox technology for shell/computer-use batteries | Battery/deployment owner during PR-056; native in-process execution remains trusted. |
| Malware/content scanning for uploaded or generated artifacts | Application owner where the threat and compliance profile requires it. |

Each choice must fail closed or remain disabled when the deployment has not configured a safe policy.

These are owned deployment inputs with fixed fail-closed framework behavior; they do not reopen the kernel, port, journal, protocol, or security-boundary design.

# 16. Residual risk register

| Residual risk | Boundary/owner | Required treatment |
|---|---|---|
| Trusted native Rust, Python, JavaScript, or host code can read process memory and subvert application policy. | Application/deployer | Minimize trusted code, isolate high-risk work, restrict credentials/OS identity, and do not describe in-process code as sandboxed. |
| Journal checksum chains detect accidental corruption and unsynchronized modification but do not authenticate history against an attacker able to rewrite records, metadata, snapshots, and the head together. | Store/deployer | Protect storage with access control/encryption/backup; use external signatures/anchoring when adversarial tamper evidence is required. |
| External model/tool/workflow effects can remain at-least-once or explicitly uncertain despite stable IDs and reconciliation. | Adapter/application | Use provider idempotency where available, expose uncertainty, and require operator/application resolution for non-repeatable outcomes. |
| Identity proofing, IdP compromise, KMS/secrets/DLP, tenant policy, retention/legal hold, and network controls remain deployment systems. | Deployer | Complete section 15 gates and operational reviews before affected features are enabled. |
| Browser local storage, memory, and UI data are exposed to the origin, device profile, extensions, and XSS within browser protections. | Browser application | Strong CSP/dependency hygiene, no provider secrets, scoped storage, explicit deletion, and same-origin proxy patterns. |
| Wasmtime, OS sandbox, container, shell, browser/computer-use, and scanning controls may contain vulnerabilities or policy gaps. | Host/battery/deployer | Patch, defense in depth, least authority, independent review, and disable privileged batteries by default. |
| SQLite acknowledgement ultimately depends on OS/filesystem/hardware honoring the documented flush contract. | Store/deployer | Use supported durable storage, test backup/restore/power-loss assumptions, surface relaxed mode, and maintain external backups. |
| Models and retrieved content remain untrusted and can still cause harmful suggestions within granted capabilities. | Product/application | Least-privilege tools, provenance, interaction/verification for sensitive actions, monitoring, and product-specific evaluation. |
| Bounded parsers/queues reduce but do not eliminate remote resource-exhaustion and traffic-amplification risk. | Server/deployer | Authentication, rate/concurrency limits, network controls, quotas, monitoring, and capacity planning. |

# 17. Traceability

| Security area | Product requirements | Architecture/TDD | Implementation |
|---|---|---|---|
| Trust classes and isolation | NFR-SEC-001/002; FR-PLG | Architecture sections 15 and 17; TDD section 27 | PR-049 through PR-054 |
| Secrets and redaction | NFR-SEC-003/005; FR-OBS | Architecture sections 17 and 19; TDD sections 19 and 31 | PR-003, PR-014, PR-022, PR-024, PR-034, PR-039, PR-057, PR-060 |
| Scoped resource authority | NFR-SEC-004; FR-TLS; FR-PLG | Architecture sections 6, 15, and 17 | PR-025, PR-052, PR-056 |
| Completion and interaction integrity | FR-KRN-013/015; FR-RT-008; FR-DUR | Architecture sections 10 and 12; TDD sections 11-13 and 23 | PR-014, PR-039, PR-042 through PR-045, PR-048 |
| Durability integrity | NFR-REL; FR-DUR | Architecture sections 10 and 20; TDD sections 18 and 23 | PR-039 through PR-048 |
| Remote protocol | UC-06; FR-RT-008; FR-DUR; NFR-COMP | Architecture sections 16-17 and 20; TDD section 28 | PR-058 |
| Binding safety | FR-PY; FR-WASM; NFR-PORT | Architecture sections 13-14; TDD sections 25-26 | PR-027 through PR-038 |
| Context compaction integrity | FR-CTX-003; FR-MW-007; NFR-SEC-003/005 | Architecture section 11.5; TDD section 17.6 | PR-018, PR-023, PR-048, PR-056/057 |
| Supply chain | PRD release criteria and risk register | Architecture section 22; TDD CI/release design | PR-003, PR-060/061, PR-065/066 |

# 18. Review triggers

This threat model must be reviewed when a change:

- adds a primary port, middleware stage, trust class, deployment topology, or privileged host capability;
- changes effect, interaction, lineage, cancellation, journal, snapshot, or terminal semantics;
- adds a protocol, parser, dynamic code-loading mechanism, network listener, browser persistence surface, or secret-bearing adapter;
- changes plugin permissions, signing, trust roots, sandboxing, or resource limits;
- changes telemetry payloads, retention, deletion, or artifact access;
- adds a high-privilege battery such as shell, computer use, filesystem write, or credential access;
- changes build/release identities, registries, signing, or provenance; or
- follows a security incident, significant vulnerability, or material deployment architecture change.
