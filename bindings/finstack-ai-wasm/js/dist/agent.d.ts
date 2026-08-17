import { Agent as WasmAgent, Event as WasmEvent, EventBatch as WasmEventBatch, Lane as WasmLane, Locator as WasmLocator, MemoryExternalIdentityMap as WasmMemoryExternalIdentityMap, Run as WasmRun, RunResult as WasmRunResult, Session as WasmSession } from "../generated/finstack_ai_wasm.js";
import type { JsContextProvider, JsJournalStore, JsMiddleware, JsModel, JsObserver, JsToolset } from "./adapters.js";
import type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
export { FinstackError } from "./errors.js";
export type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot, } from "./errors.js";
/**
 * Declarative capability delivery mode. Rust owns activation semantics.
 */
export type CapabilityActivation = "always" | "application" | "model" | "disabled";
/**
 * Instruction-only alpha capability. Native bundles may also contribute
 * registered toolsets, context providers, and middleware.
 */
export interface Capability {
    /** Namespaced capability identity. */
    id: string;
    /** Compact non-secret description. */
    description: string;
    /** Instructions contributed after activation. */
    instructions: string[];
    /**
     * Delivery mode. Defaults to `application`.
     */
    activation?: CapabilityActivation;
}
/**
 * One compact model-visible capability catalog entry.
 */
export interface CapabilityCatalogItem {
    /** Capability identity. */
    id: string;
    /** Compact non-secret description. */
    description: string;
}
/**
 * One Rust-owned capability activation committed for a run.
 */
export interface ActiveCapability {
    /** Capability identity. */
    id: string;
    /** Selecting source. */
    source: "always" | "application" | "model";
}
/**
 * Options for {@link Agent.create}.
 *
 * Host objects inherit page authority and are not a sandbox.
 */
export interface AgentOptions {
    /** Trusted JS model wrapper. */
    model: JsModel;
    /** Optional trusted toolset wrappers. */
    toolsets?: JsToolset[];
    /** Optional model instruction. */
    instruction?: string;
    /**
     * Optional host journal. When omitted, the Rust in-memory store is used.
     * Persistence remains experimental after PR-048; it does not meet NFR-REL-001.
     */
    store?: JsJournalStore;
    /**
     * Optional declarative capabilities. JS capabilities contribute instructions.
     * Pass a model-activation id to {@link Agent.start} / {@link Agent.run}
     * via run options; unknown ids fail closed.
     */
    capabilities?: Capability[];
    /**
     * Application-activated capability IDs. Model-activation IDs fail closed.
     */
    activeCapabilities?: string[];
    /** Optional trusted context-provider wrappers. */
    contextProviders?: JsContextProvider[];
    /** Optional trusted middleware wrappers. */
    middleware?: JsMiddleware[];
    /** Optional trusted observer wrappers. */
    observers?: JsObserver[];
}
/**
 * Provisional inspect phase for a stored session.
 *
 * `in_progress` and `cancelled` are valid after interrupt or reload. This is
 * not the PR-048 durable-beta crash-prefix matrix.
 */
export type SessionInspectPhase = "empty" | "in_progress" | "completed" | "failed" | "cancelled";
/**
 * Replay-derived inspect snapshot. This is not a continue credential.
 */
export interface SessionInspectSnapshot {
    /** Stored session identity. */
    sessionId: string;
    /** Current committed head; zero denotes an empty journal. */
    headSequence: number;
    /** Provisional phase reconstructed from committed records. */
    phase: SessionInspectPhase;
    /** Concatenated final assistant text when the session completed. */
    resultText?: string;
    /** Kind name of the last committed record. */
    lastRecordKind?: string;
}
/**
 * Rust-owned resolved agent handle.
 */
export declare class Agent {
    #private;
    constructor(handle: WasmAgent);
    /**
     * Construct an Agent over a trusted {@link JsModel} and optional toolsets.
     *
     * The default journal is the Rust in-memory store. Pass {@link AgentOptions.store}
     * to opt into a host journal. State remains in WASM until an explicit snapshot
     * or inspect. Reload restore is inspect, not continue-the-run.
     *
     * @param options - Model, optional toolsets, instruction, store, and capabilities.
     * @returns A resolved Agent handle.
     * @throws {FinstackError} When configuration is invalid.
     * @example
     * ```ts
     * const agent = await Agent.create({
     *   model: new JsModel(host, {
     *     component: "app.model",
     *     provider: "scripted",
     *     model: "scripted-model",
     *   }),
     *   capabilities: [{
     *     id: "app.capability.always",
     *     description: "Baseline guidance",
     *     instructions: ["Always instruction."],
     *     activation: "always",
     *   }],
     * });
     * const result = await agent.run("hello");
     * ```
     */
    static create(options: AgentOptions): Promise<Agent>;
    /**
     * Return the bounded model-activated capability catalog in identity order.
     *
     * @returns Compact catalog entries visible to model selection.
     * @example
     * ```ts
     * const catalog = agent.capabilityCatalog();
     * ```
     */
    capabilityCatalog(): CapabilityCatalogItem[];
    /**
     * Render the compact catalog supplied to model-facing integrations.
     *
     * @returns One `id: description` line per model-selectable capability.
     * @example
     * ```ts
     * const compact = agent.compactCapabilityCatalog();
     * ```
     */
    compactCapabilityCatalog(): string;
    /**
     * Replay one stored session into a provisional inspect snapshot.
     *
     * This does not continue an interrupted run or retry in-flight effects.
     *
     * @param store - Host journal that holds the session.
     * @param sessionId - Session identity to inspect.
     * @returns A JSON inspect snapshot.
     * @throws {FinstackError} When the session cannot be loaded or replayed.
     * @example
     * ```ts
     * const snapshot = await Agent.inspectSession(store, sessionId);
     * ```
     */
    static inspectSession(store: JsJournalStore, sessionId: string): Promise<SessionInspectSnapshot>;
    /**
     * Create a live session on this agent's journal store.
     *
     * @param tenantScope - Tenant scope captured by the host.
     * @returns A live session handle with a bootstrapped `main` lane.
     * @throws {FinstackError} When the session cannot be created.
     * @example
     * ```ts
     * const session = await agent.createSession("tenant-a");
     * ```
     */
    createSession(tenantScope?: string): Promise<Session>;
    /**
     * Open an existing session without respawning parked runs.
     *
     * @param sessionId - Durable session identity.
     * @param tenantScope - Tenant scope captured by the host.
     * @returns A live session handle rebuilt from the journal.
     * @throws {FinstackError} When the session cannot be opened.
     * @example
     * ```ts
     * const opened = await agent.openSession(session.sessionId, "tenant-a");
     * ```
     */
    openSession(sessionId: string, tenantScope?: string): Promise<Session>;
    /**
     * Start one run and return its detached control handle immediately.
     *
     * Dropping the returned {@link Run} detaches observation and does not cancel
     * the durable run. Call {@link Run.cancel} for explicit cancellation.
     *
     * @param input - Plain-text user input.
     * @param options - Optional timeout, cycle, retry, and capability selection.
     * @returns A shared run handle.
     * @throws {FinstackError} When the request is invalid.
     * @example
     * ```ts
     * const run = agent.start("hello");
     * const result = await run.result();
     * ```
     */
    start(input: string, options?: RunOptions): Run;
    /**
     * Execute one run and await its committed result.
     *
     * @param input - Plain-text user input.
     * @param options - Optional timeout, cycle, retry, and capability selection.
     * @returns The retained terminal result.
     * @throws {FinstackError} When the run fails, times out, or is cancelled.
     * @example
     * ```ts
     * const result = await agent.run("hello");
     * ```
     */
    run(input: string, options?: RunOptions): Promise<RunResult>;
}
/**
 * Shared control and observation handle for one Rust-owned run.
 *
 * Drop detaches observation and does not cancel execution.
 */
export declare class Run {
    #private;
    constructor(handle: WasmRun);
    /**
     * Live session handle for this run.
     */
    get session(): Session;
    /**
     * Immutable operation locator snapshot.
     */
    get locator(): Locator;
    /**
     * Batch-first asynchronous event iterator.
     *
     * {@link EventOptions.signal} cancels iteration and closes delivery only.
     * It does not cancel the durable run.
     *
     * @param options - Optional abort signal for the iterator.
     * @returns An async iterable of {@link EventBatch} values.
     * @throws {FinstackError} When event delivery fails to start.
     * @example
     * ```ts
     * for await (const batch of run.events()) {
     *   const first = batch.firstSequence;
     * }
     * ```
     */
    events(options?: EventOptions): AsyncIterable<EventBatch>;
    /**
     * Wait for the retained terminal result.
     *
     * @returns The committed result.
     * @throws {FinstackError} When the run fails, times out, or is cancelled.
     * @example
     * ```ts
     * const result = await run.result();
     * ```
     */
    result(): Promise<RunResult>;
    /**
     * Submit idempotent durable cancellation.
     *
     * @param reason - Optional non-secret reason. Not persisted as raw host text.
     * @returns A promise that settles when cancellation is accepted.
     * @throws {FinstackError} When cancellation cannot be committed.
     * @example
     * ```ts
     * await run.cancel();
     * await run.cancel();
     * ```
     */
    cancel(reason?: string): Promise<void>;
    /**
     * Close event delivery without cancelling the run.
     *
     * @returns A promise that settles after delivery is closed.
     * @example
     * ```ts
     * await run.closeEvents();
     * const result = await run.result();
     * ```
     */
    closeEvents(): Promise<void>;
}
/**
 * Immutable identifiers for one accepted operation.
 */
export declare class Locator {
    #private;
    constructor(handle: WasmLocator);
    /** Tenant scope captured at acceptance. */
    get tenantScope(): string;
    /** Session identity. */
    get sessionId(): string;
    /** Lane identity. */
    get laneId(): string;
    /** Run identity. */
    get runId(): string;
    /**
     * Serialize the identifier snapshot explicitly.
     *
     * @returns A plain locator object.
     */
    toDict(): SessionSnapshot;
}
/**
 * Inspect snapshot for one live lane.
 */
export interface LaneInspectSnapshot {
    /** Durable lane identity. */
    laneId: string;
    /** Stable application name. */
    name: string;
    /** History length ending at the current leaf. */
    historyLen: number;
}
/**
 * Host-owned `(channel, account, thread)` resolution.
 */
export interface ExternalIdentitySnapshot {
    /** Bound session identity. */
    sessionId: string;
    /** Bound lane identity. */
    laneId: string;
}
/**
 * Live handle for one journaled session.
 */
export declare class Session {
    #private;
    constructor(handle: WasmSession);
    /** Tenant scope captured by the host. */
    get tenantScope(): string;
    /** Session identity. */
    get sessionId(): string;
    /**
     * Create a named lane, optionally forking from an existing entry.
     *
     * @param name - Application lane name. `main` is reserved for bootstrap.
     * @param fork - Optional existing entry identity to share without copying.
     * @returns The new live lane handle.
     * @throws {FinstackError} When the name is invalid or the fork is unknown.
     * @example
     * ```ts
     * const research = await session.createLane("research", leafId);
     * ```
     */
    createLane(name: string, fork?: string): Promise<Lane>;
    /**
     * List restored lanes.
     *
     * @returns Live handles for every lane in the session.
     * @throws {FinstackError} When the session cannot be loaded.
     * @example
     * ```ts
     * const lanes = await session.listLanes();
     * ```
     */
    listLanes(): Promise<Lane[]>;
    /**
     * Look up one lane by application name.
     *
     * @param name - Application lane name.
     * @returns The live lane handle.
     * @throws {FinstackError} When the lane does not exist.
     * @example
     * ```ts
     * const main = await session.lane("main");
     * ```
     */
    lane(name: string): Promise<Lane>;
    /**
     * Bind a host-owned external identity to one lane.
     *
     * @param map - In-process identity map owned by the host.
     * @param channel - Channel or adapter name.
     * @param account - Account identity on that channel.
     * @param thread - Conversation or thread identity.
     * @param laneId - Durable lane identity in this session.
     * @throws {FinstackError} When the key conflicts or the lane is unknown.
     * @example
     * ```ts
     * const map = new MemoryExternalIdentityMap();
     * session.bindExternalIdentity(map, "slack", "acct", "thread-1", main.laneId);
     * ```
     */
    bindExternalIdentity(map: MemoryExternalIdentityMap, channel: string, account: string, thread: string, laneId: string): void;
}
/**
 * Live handle for one lane in a session.
 */
export declare class Lane {
    #private;
    constructor(handle: WasmLane);
    /** Durable lane identity. */
    get laneId(): string;
    /** Session that owns this lane. */
    get session(): Session;
    /**
     * Point this idle lane at an existing entry without copying.
     *
     * @param entryId - Existing conversation entry identity.
     * @throws {FinstackError} When the entry is unknown or the lane is busy.
     * @example
     * ```ts
     * await research.navigate(leafId);
     * ```
     */
    navigate(entryId: string): Promise<void>;
    /**
     * Inspect name, leaf, and history length.
     *
     * @returns A snapshot of the restored lane.
     * @throws {FinstackError} When the lane cannot be inspected.
     * @example
     * ```ts
     * const inspect = await research.inspect();
     * ```
     */
    inspect(): Promise<LaneInspectSnapshot>;
}
/**
 * In-process external identity map.
 */
export declare class MemoryExternalIdentityMap {
    constructor(handle?: WasmMemoryExternalIdentityMap);
    /**
     * Resolve one previously bound key.
     *
     * @param channel - Channel or adapter name.
     * @param account - Account identity on that channel.
     * @param thread - Conversation or thread identity.
     * @returns The bound session and lane, or `undefined`.
     * @throws {FinstackError} When the key is invalid.
     * @example
     * ```ts
     * const bound = map.resolve("slack", "acct", "thread-1");
     * ```
     */
    resolve(channel: string, account: string, thread: string): ExternalIdentitySnapshot | undefined;
}
/**
 * Immutable successful terminal result snapshot.
 */
export declare class RunResult {
    #private;
    constructor(handle: WasmRunResult);
    /** Concatenated final assistant text. */
    get text(): string;
    /** Durable retry attempts consumed by this run. */
    get retryAttempts(): number;
    /**
     * Stable Rust-owned committed record-kind trace in journal order.
     */
    get trace(): string[];
    /**
     * Complete Rust-owned capability activation set for this run.
     */
    get activeCapabilities(): ActiveCapability[];
    /** Locator snapshot for the completed run. */
    get locator(): Locator;
    /** Locator snapshot for the completed run. */
    get session(): Locator;
    /**
     * Serialize the terminal result explicitly.
     *
     * @returns Locator fields plus `text`.
     * @example
     * ```ts
     * const snapshot = result.toDict();
     * ```
     */
    toDict(): RunResultSnapshot;
}
/**
 * Immutable runtime event handle. Getters do not serialize the world.
 */
export declare class Event {
    #private;
    constructor(handle: WasmEvent);
    /** Event kind name such as `run_completed`. */
    get kind(): string;
    /** Durable or transient class. */
    get eventClass(): string;
    /** Transient sequence. */
    get transientSequence(): number;
    /** Durable sequence, when the event is durable-derived. */
    get durableSequence(): number | undefined;
    /**
     * Serialize the complete event explicitly.
     *
     * @returns Canonical JSON text.
     * @example
     * ```ts
     * const json = event.toJson();
     * ```
     */
    toJson(): string;
}
/**
 * Immutable bounded transport batch.
 *
 * Iterate the batch without calling {@link EventBatch.events} to avoid
 * per-event FFI. Expand only when individual events are required.
 */
export declare class EventBatch {
    #private;
    constructor(handle: WasmEventBatch);
    /** First contained sequence. */
    get firstSequence(): number;
    /** Last contained sequence. */
    get lastSequence(): number;
    /** Lag-dropped transient events since the previous batch. */
    get droppedProgress(): number;
    /**
     * Expand the batch into individual immutable event handles.
     *
     * @returns Event handles for this transport batch.
     * @example
     * ```ts
     * const events = batch.events();
     * ```
     */
    events(): Event[];
    /**
     * Serialize the complete batch explicitly.
     *
     * @returns Canonical JSON text of every contained event.
     * @example
     * ```ts
     * const json = batch.toJson();
     * ```
     */
    toJson(): string;
    /**
     * Serialize the complete batch once and copy it into bytes.
     *
     * @returns UTF-8 JSON for every event in this transport batch.
     * @example
     * ```ts
     * const bytes = batch.toJsonBytes();
     * ```
     */
    toJsonBytes(): Uint8Array;
}
//# sourceMappingURL=agent.d.ts.map