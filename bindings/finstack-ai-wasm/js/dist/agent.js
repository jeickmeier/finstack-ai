import { Agent as WasmAgent, Event as WasmEvent, EventBatch as WasmEventBatch, Run as WasmRun, RunResult as WasmRunResult, Session as WasmSession, } from "../generated/finstack_ai_wasm.js";
import { requireWasm, wasmJournalStoreHandle, wasmModelHandle, wasmToolsetHandle, } from "./adapters.js";
import { FinstackError } from "./errors.js";
export { FinstackError } from "./errors.js";
/**
 * Rust-owned resolved agent handle.
 */
export class Agent {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
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
    static async create(options) {
        requireWasm();
        try {
            const handle = await WasmAgent.create(wasmModelHandle(options.model), (options.toolsets ?? []).map((toolset) => wasmToolsetHandle(toolset).cloneHandle()), options.instruction, options.store === undefined
                ? undefined
                : wasmJournalStoreHandle(options.store).cloneHandle(), options.capabilities === undefined
                ? undefined
                : JSON.stringify(options.capabilities), options.activeCapabilities === undefined
                ? undefined
                : JSON.stringify(options.activeCapabilities));
            return new Agent(handle);
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
    /**
     * Return the bounded model-activated capability catalog in identity order.
     *
     * @returns Compact catalog entries visible to model selection.
     * @example
     * ```ts
     * const catalog = agent.capabilityCatalog();
     * ```
     */
    capabilityCatalog() {
        requireWasm();
        return this.#handle.capabilityCatalog();
    }
    /**
     * Render the compact catalog supplied to model-facing integrations.
     *
     * @returns One `id: description` line per model-selectable capability.
     * @example
     * ```ts
     * const compact = agent.compactCapabilityCatalog();
     * ```
     */
    compactCapabilityCatalog() {
        requireWasm();
        return this.#handle.compactCapabilityCatalog();
    }
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
    static async inspectSession(store, sessionId) {
        requireWasm();
        try {
            const snapshot = (await WasmAgent.inspectSession(wasmJournalStoreHandle(store), sessionId));
            return {
                ...snapshot,
                headSequence: Number(snapshot.headSequence),
            };
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
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
    start(input, options) {
        requireWasm();
        try {
            return new Run(this.#handle.start(input, options?.timeoutSeconds, options?.maxCycles, options?.maxOutputRetries));
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
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
    async run(input, options) {
        requireWasm();
        try {
            const handle = await this.#handle.run(input, options?.timeoutSeconds, options?.maxCycles, options?.maxOutputRetries);
            return new RunResult(handle);
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
}
/**
 * Shared control and observation handle for one Rust-owned run.
 *
 * Drop detaches observation and does not cancel execution.
 */
export class Run {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
    /**
     * Immutable session locator for this run.
     */
    get session() {
        return new Session(this.#handle.session);
    }
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
    events(options) {
        const handle = this.#handle;
        const signal = options?.signal;
        return {
            [Symbol.asyncIterator]() {
                let closed = signal?.aborted === true;
                const onAbort = () => {
                    closed = true;
                    handle.closeEvents();
                };
                signal?.addEventListener("abort", onAbort, { once: true });
                return {
                    async next() {
                        if (closed || signal?.aborted) {
                            return { done: true, value: undefined };
                        }
                        try {
                            const batch = await handle.nextEventBatch();
                            if (batch === undefined) {
                                return { done: true, value: undefined };
                            }
                            return { done: false, value: new EventBatch(batch) };
                        }
                        catch (error) {
                            throw FinstackError.fromUnknown(error);
                        }
                    },
                    async return() {
                        closed = true;
                        signal?.removeEventListener("abort", onAbort);
                        await handle.closeEvents();
                        return { done: true, value: undefined };
                    },
                };
            },
        };
    }
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
    async result() {
        try {
            return new RunResult(await this.#handle.result());
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
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
    async cancel(reason) {
        try {
            await this.#handle.cancel(reason);
        }
        catch (error) {
            throw FinstackError.fromUnknown(error);
        }
    }
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
    async closeEvents() {
        await this.#handle.closeEvents();
    }
}
/**
 * Immutable identifiers for one accepted operation.
 */
export class Session {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
    /** Tenant scope captured at acceptance. */
    get tenantScope() {
        return this.#handle.tenantScope;
    }
    /** Session identity. */
    get sessionId() {
        return this.#handle.sessionId;
    }
    /** Lane identity. */
    get laneId() {
        return this.#handle.laneId;
    }
    /** Run identity. */
    get runId() {
        return this.#handle.runId;
    }
    /**
     * Serialize the identifier snapshot explicitly.
     *
     * @returns A plain locator object.
     * @example
     * ```ts
     * const snapshot = run.session.toDict();
     * ```
     */
    toDict() {
        return this.#handle.toDict();
    }
}
/**
 * Immutable successful terminal result snapshot.
 */
export class RunResult {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
    /** Concatenated final assistant text. */
    get text() {
        return this.#handle.text;
    }
    /** Durable retry attempts consumed by this run. */
    get retryAttempts() {
        return this.#handle.retryAttempts;
    }
    /**
     * Stable Rust-owned committed record-kind trace in journal order.
     */
    get trace() {
        return Array.from(this.#handle.trace);
    }
    /**
     * Complete Rust-owned capability activation set for this run.
     */
    get activeCapabilities() {
        return this.#handle.activeCapabilities;
    }
    /** Session locator for the completed run. */
    get session() {
        return new Session(this.#handle.session);
    }
    /**
     * Serialize the terminal result explicitly.
     *
     * @returns Locator fields plus `text`.
     * @example
     * ```ts
     * const snapshot = result.toDict();
     * ```
     */
    toDict() {
        return this.#handle.toDict();
    }
}
/**
 * Immutable runtime event handle. Getters do not serialize the world.
 */
export class Event {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
    /** Event kind name such as `run_completed`. */
    get kind() {
        return this.#handle.kind;
    }
    /** Durable or transient class. */
    get eventClass() {
        return this.#handle.eventClass;
    }
    /** Transient sequence. */
    get transientSequence() {
        return asNumber(this.#handle.transientSequence);
    }
    /** Durable sequence, when the event is durable-derived. */
    get durableSequence() {
        const value = this.#handle.durableSequence;
        return value === undefined || value === null ? undefined : asNumber(value);
    }
    /**
     * Serialize the complete event explicitly.
     *
     * @returns Canonical JSON text.
     * @example
     * ```ts
     * const json = event.toJson();
     * ```
     */
    toJson() {
        return this.#handle.toJson();
    }
}
/**
 * Immutable bounded transport batch.
 *
 * Iterate the batch without calling {@link EventBatch.events} to avoid
 * per-event FFI. Expand only when individual events are required.
 */
export class EventBatch {
    #handle;
    constructor(handle) {
        this.#handle = handle;
    }
    /** First contained sequence. */
    get firstSequence() {
        return asNumber(this.#handle.firstSequence);
    }
    /** Last contained sequence. */
    get lastSequence() {
        return asNumber(this.#handle.lastSequence);
    }
    /** Lag-dropped transient events since the previous batch. */
    get droppedProgress() {
        return asNumber(this.#handle.droppedProgress);
    }
    /**
     * Expand the batch into individual immutable event handles.
     *
     * @returns Event handles for this transport batch.
     * @example
     * ```ts
     * const events = batch.events();
     * ```
     */
    events() {
        return this.#handle.events().map((event) => new Event(event));
    }
    /**
     * Serialize the complete batch explicitly.
     *
     * @returns Canonical JSON text of every contained event.
     * @example
     * ```ts
     * const json = batch.toJson();
     * ```
     */
    toJson() {
        return this.#handle.toJson();
    }
    /**
     * Serialize the complete batch once and copy it into bytes.
     *
     * @returns UTF-8 JSON for every event in this transport batch.
     * @example
     * ```ts
     * const bytes = batch.toJsonBytes();
     * ```
     */
    toJsonBytes() {
        return this.#handle.toJsonBytes();
    }
}
function asNumber(value) {
    return typeof value === "bigint" ? Number(value) : value;
}
//# sourceMappingURL=agent.js.map