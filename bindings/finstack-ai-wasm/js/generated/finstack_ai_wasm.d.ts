/* tslint:disable */
/* eslint-disable */

/**
 * Rust-owned Agent handle.
 */
export class Agent {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Return the bounded model-activated capability catalog in identity order.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the catalog object cannot be constructed.
     */
    capabilityCatalog(): any;
    /**
     * Render the compact catalog supplied to model-facing integrations.
     */
    compactCapabilityCatalog(): string;
    /**
     * Construct an Agent over a trusted JS model and optional toolsets.
     *
     * Linked provider constructors (`openai`, `anthropic`, and peers) are
     * native-only. Browser hosts use this method with a JS model adapter.
     * `approval_grant` accepts `per_call` (default) or `informed_batch`.
     *
     * # Errors
     *
     * Returns a structured host error when configuration is invalid.
     */
    static create(model: JsModel, toolsets: JsToolset[], instruction?: string | null, store?: JsJournalStore | null, capabilities_json?: string | null, active_capabilities_json?: string | null, context_providers?: JsContextProvider[] | null, middleware?: JsMiddleware[] | null, observers?: JsObserver[] | null, approval_grant?: string | null): Promise<any>;
    /**
     * Create a live session on this agent's journal store.
     *
     * # Errors
     *
     * Returns a structured host error when the session cannot be created.
     */
    createSession(tenant_scope?: string | null): Promise<any>;
    /**
     * Replay one stored session into a provisional inspect snapshot.
     *
     * This does not continue an interrupted run or retry in-flight effects.
     *
     * # Errors
     *
     * Returns a structured host error when the session id is invalid or the
     * stored journal cannot be replayed.
     */
    static inspectSession(store: JsJournalStore, session_id: string): Promise<any>;
    /**
     * Open an existing session without respawning parked runs.
     *
     * # Errors
     *
     * Returns a structured host error when the session id is invalid or the
     * stored journal cannot be replayed.
     */
    openSession(session_id: string, tenant_scope?: string | null): Promise<any>;
    /**
     * Compose a new agent from reconstructed catalogs.
     *
     * wasm-host maps the same Rust method. Missing reconstruct support fails
     * closed from Rust.
     */
    reResolve(): Promise<any>;
    /**
     * Execute one run and await its committed result.
     *
     * # Errors
     *
     * Returns a structured host error when the run fails.
     */
    run(input: string, timeout_seconds: number | null | undefined, max_cycles: number | null | undefined, max_output_retries: number | null | undefined, capability: string | null | undefined, attachments: any): Promise<any>;
    /**
     * Start one run and return its detached control handle.
     *
     * # Errors
     *
     * Returns a structured host error when the request is invalid.
     */
    start(input: string, timeout_seconds: number | null | undefined, max_cycles: number | null | undefined, max_output_retries: number | null | undefined, capability: string | null | undefined, attachments: any): Run;
}

/**
 * Immutable runtime event handle.
 */
export class Event {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Explicit JSON snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the event cannot be serialized.
     */
    toJson(): string;
    /**
     * Durable sequence, when the event is durable-derived.
     */
    readonly durableSequence: bigint | undefined;
    /**
     * Durable or transient class.
     */
    readonly eventClass: string;
    /**
     * Event kind name.
     */
    readonly kind: string;
    /**
     * Transient sequence.
     */
    readonly transientSequence: bigint;
}

/**
 * Bounded transport batch. Expand events only on request.
 */
export class EventBatch {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Expand contained events. This is the per-event FFI boundary.
     */
    events(): Event[];
    /**
     * Explicit JSON snapshot of the contained events.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the batch cannot be serialized.
     */
    toJson(): string;
    /**
     * Explicit UTF-8 JSON bytes of the contained events.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the batch cannot be serialized.
     */
    toJsonBytes(): Uint8Array;
    /**
     * Lag-dropped transient events since the previous batch.
     */
    readonly droppedProgress: bigint;
    /**
     * First contained sequence.
     */
    readonly firstSequence: bigint;
    /**
     * Last contained sequence.
     */
    readonly lastSequence: bigint;
}

/**
 * Host artifact-store wrapper over `Uint8Array` payloads.
 */
export class JsArtifactStore {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct an artifact-store wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `stagePut` or `get` is missing.
     */
    constructor(adapter: any);
}

/**
 * Host clock wrapper. `now()` returns Unix milliseconds.
 */
export class JsClock {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a clock wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `now` is missing.
     */
    constructor(adapter: any);
}

/**
 * Trusted JS context-provider wrapper.
 */
export class JsContextProvider {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a context-provider wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Clone the wrapper without moving the caller's handle.
     */
    cloneHandle(): JsJournalStore;
    /**
     * Construct a journal-store wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when the adapter is invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS memory-store wrapper. Missing `memory_*` methods on the
 * adapter are `Unavailable` per operation, not a construction failure.
 */
export class JsMemoryStore {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Clone the wrapper without moving the caller's handle.
     */
    cloneHandle(): JsMemoryStore;
    /**
     * Construct a memory-store wrapper around a trusted host adapter.
     */
    constructor(adapter: any);
}

/**
 * Trusted JS middleware wrapper.
 */
export class JsMiddleware {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a middleware wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export class JsModel {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a model wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS observer wrapper.
 */
export class JsObserver {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct an observer wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Host random-source wrapper.
 */
export class JsRandomSource {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a random-source wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `fillBytes` is missing.
     */
    constructor(adapter: any);
}

/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export class JsToolset {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Clone the wrapper without moving the caller's handle.
     */
    cloneHandle(): JsToolset;
    /**
     * Construct a toolset wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Live lane handle.
 */
export class Lane {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Inspect name, leaf, and history length.
     *
     * # Errors
     *
     * Returns a structured host error when the lane cannot be inspected.
     */
    inspect(): Promise<any>;
    /**
     * Point this idle lane at an existing entry without copying.
     *
     * # Errors
     *
     * Returns a structured host error when the entry is unknown or the lane
     * is busy.
     */
    navigate(entry_id: string): Promise<any>;
    /**
     * Resume is unsupported on wasm-host; there is no truthful respawn path.
     *
     * # Errors
     *
     * Always returns `agent_run_unsupported_plan`.
     */
    resume(_agent: Agent): Promise<any>;
    /**
     * Start a new root run on this idle lane.
     *
     * # Errors
     *
     * Returns a structured host error when the lane is busy or the agent
     * cannot start.
     */
    run(agent: Agent, input: string, timeout_seconds: number | null | undefined, max_cycles: number | null | undefined, max_output_retries: number | null | undefined, capability: string | null | undefined, attachments: any): Run;
    /**
     * Park is unsupported on wasm-host; there is no truthful respawn path.
     *
     * # Errors
     *
     * Always returns `agent_run_unsupported_plan`.
     */
    suspend(): Promise<any>;
    /**
     * Durable lane identity.
     */
    readonly laneId: string;
    /**
     * Session that owns this lane.
     */
    readonly session: Session;
}

/**
 * Read-only operation locator.
 */
export class Locator {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Explicit locator snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the snapshot object cannot be constructed.
     */
    toDict(): any;
    /**
     * Lane identity.
     */
    readonly laneId: string;
    /**
     * Run identity.
     */
    readonly runId: string;
    /**
     * Session identity.
     */
    readonly sessionId: string;
    /**
     * Tenant scope captured at acceptance.
     */
    readonly tenantScope: string;
}

/**
 * In-process external identity map.
 */
export class MemoryExternalIdentityMap {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Empty map.
     */
    constructor();
    /**
     * Resolve one previously bound key.
     *
     * # Errors
     *
     * Returns a structured host error when the key is invalid.
     */
    resolve(channel: string, account: string, thread: string): any;
}

/**
 * Detached run control handle. Drop detaches observation and does not cancel.
 *
 * `list_interactions` / `resolve_interaction` remain native-only
 * (`native-tokio`). Browser WASM uses the host session/inbox path.
 */
export class Run {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Submit idempotent durable cancellation.
     *
     * # Errors
     *
     * Returns a structured host error when cancellation cannot be committed.
     */
    cancel(_reason?: string | null): Promise<any>;
    /**
     * Close event delivery without cancelling the run.
     */
    closeEvents(): Promise<any>;
    /**
     * Receive the next transport batch, or `undefined` after close/terminal.
     *
     * # Errors
     *
     * Returns a structured host error when event delivery fails to start.
     */
    nextEventBatch(): Promise<any>;
    /**
     * Wait for the retained terminal result.
     *
     * # Errors
     *
     * Returns a structured host error when the run fails, times out, or is cancelled.
     */
    result(): Promise<any>;
    /**
     * Prepare and accept one child through the Rust router.
     *
     * wasm-host fails closed with `agent_run_unsupported_plan`.
     */
    startChild(child: Agent, input: string, placement: string, route_endpoint?: string | null, route_service?: string | null, route_id?: string | null, route_token?: string | null): Promise<any>;
    /**
     * Immutable operation locator snapshot.
     */
    readonly locator: Locator;
    /**
     * Live session handle for this run.
     */
    readonly session: Session;
}

/**
 * Successful terminal result handle.
 */
export class RunResult {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Explicit result snapshot.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the snapshot object cannot be constructed.
     */
    toDict(): any;
    /**
     * Complete Rust-owned capability activation set for this run.
     *
     * # Errors
     *
     * Returns a JavaScript exception when the activation objects cannot be constructed.
     */
    readonly activeCapabilities: any;
    /**
     * Operation locator for the completed run.
     */
    readonly locator: Locator;
    /**
     * Durable retry attempts consumed by this run.
     */
    readonly retryAttempts: number;
    /**
     * Locator snapshot for the completed run.
     */
    readonly session: Locator;
    /**
     * Concatenated final assistant text.
     */
    readonly text: string;
    /**
     * Stable Rust-owned committed record-kind trace in journal order.
     */
    readonly trace: string[];
}

/**
 * Live session handle.
 */
export class Session {
    private constructor();
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Bind a host-owned external identity to one lane.
     *
     * # Errors
     *
     * Returns a structured host error when the key is invalid, the lane is
     * unknown, or the key is already bound to a different session lane.
     */
    bindExternalIdentity(map: MemoryExternalIdentityMap, channel: string, account: string, thread: string, lane_id: string): void;
    /**
     * Create a named lane, optionally forking from an existing entry.
     *
     * # Errors
     *
     * Returns a structured host error when the lane cannot be created.
     */
    createLane(name: string, fork?: string | null): Promise<any>;
    /**
     * Look up one lane by application name.
     *
     * # Errors
     *
     * Returns a structured host error when the lane does not exist.
     */
    lane(name: string): Promise<any>;
    /**
     * List restored lanes.
     *
     * # Errors
     *
     * Returns a structured host error when the session cannot be loaded.
     */
    listLanes(): Promise<any>;
    /**
     * Session identity.
     */
    readonly sessionId: string;
    /**
     * Tenant scope captured by the host.
     */
    readonly tenantScope: string;
}

/**
 * Apply normalized coordinator commands and return identity traces.
 *
 * This export is test-only and does not submit a live Agent.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when any command fails DTO validation.
 */
export function applyScriptedCoordinatorCommands(encoded: string): string;

/**
 * Lockstep version metadata for the wasm package.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the metadata object cannot be constructed.
 */
export function buildMetadata(): any;

/**
 * Construct the six-port compile fixtures for the current target.
 *
 * Test-only: compiled for `cargo test` and the `scripted-trace` wasm harness.
 */
export function compilePortProxies(): void;

/**
 * Drive scripted journal-store health. Test-only; always non-durable.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the adapter is invalid.
 */
export function driveScriptedJournalHealth(adapter: any, options: any): Promise<any>;

/**
 * Drive one scripted `Model::request`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 */
export function driveScriptedModelRequest(adapter: any, options: any, signal: any): Promise<any>;

/**
 * Drive one scripted `Toolset::call`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 */
export function driveScriptedToolCall(adapter: any, options: any, signal: any): Promise<any>;

/**
 * Process-local health token. Does not create a runtime, open a store, or spawn work.
 */
export function health(): string;

/**
 * Compute journal known-answer hex through the one Rust engine.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when `kind` or the diagnostic JSON is invalid.
 */
export function journalKnownAnswer(kind: string, encoded: string): string;

/**
 * Normalize a pre-beta lineage or authenticated external-command shape.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when `kind` is unsupported or `value` is invalid.
 */
export function normalizePrebetaShape(kind: string, encoded: string): string;

/**
 * Debug helper: parse a document and return the full detailed result.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent carrying the stable `document_*` parse
 * error code when the input is oversized, unsupported, or unparseable, or
 * when the parsed result cannot be serialized.
 */
export function parseDocument(data: Uint8Array, media_type: string): any;

/**
 * Debug helper: parse a document to Markdown and return only the Markdown.
 *
 * Runs the same `finstack-ai-tools-document` parser the ingest middleware
 * uses, without an `Agent` or `Run`, so a developer can see exactly what
 * would be injected for a given file.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent carrying the stable `document_*` parse
 * error code when the input is oversized, unsupported, or unparseable.
 */
export function parseDocumentMarkdown(data: Uint8Array, media_type: string): string;

/**
 * Run the embedded no-op golden trace through `CommitCoordinator`.
 *
 * This export is a test-only engine-load proof and is not part of the public
 * TypeScript surface.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the fixture, store, or report fails.
 */
export function runNoopTrace(): string;

/**
 * Install the host driver when the generated module loads.
 */
export function wasm_start(): void;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_agent_free: (a: number, b: number) => void;
    readonly __wbg_event_free: (a: number, b: number) => void;
    readonly __wbg_eventbatch_free: (a: number, b: number) => void;
    readonly __wbg_jsartifactstore_free: (a: number, b: number) => void;
    readonly __wbg_jsclock_free: (a: number, b: number) => void;
    readonly __wbg_jscontextprovider_free: (a: number, b: number) => void;
    readonly __wbg_jsjournalstore_free: (a: number, b: number) => void;
    readonly __wbg_jsmemorystore_free: (a: number, b: number) => void;
    readonly __wbg_jsmiddleware_free: (a: number, b: number) => void;
    readonly __wbg_jsmodel_free: (a: number, b: number) => void;
    readonly __wbg_jsobserver_free: (a: number, b: number) => void;
    readonly __wbg_jstoolset_free: (a: number, b: number) => void;
    readonly __wbg_lane_free: (a: number, b: number) => void;
    readonly __wbg_locator_free: (a: number, b: number) => void;
    readonly __wbg_memoryexternalidentitymap_free: (a: number, b: number) => void;
    readonly __wbg_run_free: (a: number, b: number) => void;
    readonly __wbg_runresult_free: (a: number, b: number) => void;
    readonly __wbg_session_free: (a: number, b: number) => void;
    readonly agent_capabilityCatalog: (a: number, b: number) => void;
    readonly agent_compactCapabilityCatalog: (a: number, b: number) => void;
    readonly agent_create: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number, o: number, p: number, q: number, r: number) => number;
    readonly agent_createSession: (a: number, b: number, c: number) => number;
    readonly agent_inspectSession: (a: number, b: number, c: number) => number;
    readonly agent_openSession: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly agent_reResolve: (a: number) => number;
    readonly agent_run: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number) => number;
    readonly agent_start: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number) => void;
    readonly applyScriptedCoordinatorCommands: (a: number, b: number, c: number) => void;
    readonly buildMetadata: (a: number) => void;
    readonly compilePortProxies: () => void;
    readonly event_durableSequence: (a: number, b: number) => void;
    readonly event_eventClass: (a: number, b: number) => void;
    readonly event_kind: (a: number, b: number) => void;
    readonly event_toJson: (a: number, b: number) => void;
    readonly event_transientSequence: (a: number) => bigint;
    readonly eventbatch_droppedProgress: (a: number) => bigint;
    readonly eventbatch_events: (a: number, b: number) => void;
    readonly eventbatch_firstSequence: (a: number) => bigint;
    readonly eventbatch_lastSequence: (a: number) => bigint;
    readonly eventbatch_toJson: (a: number, b: number) => void;
    readonly eventbatch_toJsonBytes: (a: number, b: number) => void;
    readonly health: (a: number) => void;
    readonly journalKnownAnswer: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly jsartifactstore_new: (a: number, b: number) => void;
    readonly jsclock_new: (a: number, b: number) => void;
    readonly jscontextprovider_new: (a: number, b: number, c: number) => void;
    readonly jsjournalstore_cloneHandle: (a: number) => number;
    readonly jsjournalstore_new: (a: number, b: number, c: number) => void;
    readonly jsmemorystore_cloneHandle: (a: number) => number;
    readonly jsmemorystore_new: (a: number) => number;
    readonly jsmiddleware_new: (a: number, b: number, c: number) => void;
    readonly jsmodel_new: (a: number, b: number, c: number) => void;
    readonly jsobserver_new: (a: number, b: number, c: number) => void;
    readonly jsrandomsource_new: (a: number, b: number) => void;
    readonly jstoolset_cloneHandle: (a: number) => number;
    readonly jstoolset_new: (a: number, b: number, c: number) => void;
    readonly lane_inspect: (a: number) => number;
    readonly lane_laneId: (a: number, b: number) => void;
    readonly lane_navigate: (a: number, b: number, c: number) => number;
    readonly lane_resume: (a: number, b: number) => number;
    readonly lane_run: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number) => void;
    readonly lane_session: (a: number) => number;
    readonly lane_suspend: (a: number) => number;
    readonly locator_laneId: (a: number, b: number) => void;
    readonly locator_runId: (a: number, b: number) => void;
    readonly locator_sessionId: (a: number, b: number) => void;
    readonly locator_tenantScope: (a: number, b: number) => void;
    readonly locator_toDict: (a: number, b: number) => void;
    readonly memoryexternalidentitymap_new: () => number;
    readonly memoryexternalidentitymap_resolve: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number) => void;
    readonly normalizePrebetaShape: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly parseDocument: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly parseDocumentMarkdown: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly runNoopTrace: (a: number) => void;
    readonly run_cancel: (a: number, b: number, c: number) => number;
    readonly run_closeEvents: (a: number) => number;
    readonly run_locator: (a: number) => number;
    readonly run_nextEventBatch: (a: number) => number;
    readonly run_result: (a: number) => number;
    readonly run_session: (a: number) => number;
    readonly run_startChild: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number, l: number, m: number, n: number) => number;
    readonly runresult_activeCapabilities: (a: number, b: number) => void;
    readonly runresult_locator: (a: number) => number;
    readonly runresult_retryAttempts: (a: number) => number;
    readonly runresult_text: (a: number, b: number) => void;
    readonly runresult_toDict: (a: number, b: number) => void;
    readonly runresult_trace: (a: number, b: number) => void;
    readonly session_bindExternalIdentity: (a: number, b: number, c: number, d: number, e: number, f: number, g: number, h: number, i: number, j: number, k: number) => void;
    readonly session_createLane: (a: number, b: number, c: number, d: number, e: number) => number;
    readonly session_lane: (a: number, b: number, c: number) => number;
    readonly session_listLanes: (a: number) => number;
    readonly session_sessionId: (a: number, b: number) => void;
    readonly session_tenantScope: (a: number, b: number) => void;
    readonly wasm_start: () => void;
    readonly driveScriptedModelRequest: (a: number, b: number, c: number) => number;
    readonly driveScriptedToolCall: (a: number, b: number, c: number) => number;
    readonly runresult_session: (a: number) => number;
    readonly driveScriptedJournalHealth: (a: number, b: number) => number;
    readonly __wbg_jsrandomsource_free: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_5056: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_5070: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_434: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export5: (a: number, b: number, c: number) => void;
    readonly __wbindgen_start: () => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
