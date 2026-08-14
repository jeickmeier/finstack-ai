import { Agent as WasmAgent, Event as WasmEvent, EventBatch as WasmEventBatch, Run as WasmRun, RunResult as WasmRunResult, Session as WasmSession } from "../generated/finstack_ai_wasm.js";
import type { JsModel, JsToolset } from "./adapters.js";
import type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
export { FinstackError } from "./errors.js";
export type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot, } from "./errors.js";
/**
 * Options for {@link Agent.create}.
 */
export interface AgentOptions {
    /** Trusted JS model wrapper. */
    model: JsModel;
    /** Optional trusted toolset wrappers. */
    toolsets?: JsToolset[];
    /** Optional model instruction. */
    instruction?: string;
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
     * The default journal is the Rust in-memory store. This method does not take
     * a JavaScript journal. State remains in WASM until an explicit snapshot.
     *
     * @param options - Model, optional toolsets, and optional instruction.
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
     * });
     * const result = await agent.run("hello");
     * ```
     */
    static create(options: AgentOptions): Promise<Agent>;
    /**
     * Start one run and return its detached control handle immediately.
     *
     * Dropping the returned {@link Run} detaches observation and does not cancel
     * the durable run. Call {@link Run.cancel} for explicit cancellation.
     *
     * @param input - Plain-text user input.
     * @param options - Optional timeout, cycle, and retry bounds.
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
     * @param options - Optional timeout, cycle, and retry bounds.
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
     * Immutable session locator for this run.
     */
    get session(): Session;
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
export declare class Session {
    #private;
    constructor(handle: WasmSession);
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
     * @example
     * ```ts
     * const snapshot = run.session.toDict();
     * ```
     */
    toDict(): SessionSnapshot;
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
    /** Session locator for the completed run. */
    get session(): Session;
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