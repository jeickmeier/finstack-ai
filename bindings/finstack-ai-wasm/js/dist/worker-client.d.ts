import type { RunStateSnapshot, SessionInspectSnapshot } from "./agent.js";
import { FinstackError } from "./errors.js";
import type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
import { type LagPolicy, type MainToWorker, type WorkerToMain } from "./worker-protocol.js";
/**
 * Options for {@link connectWorker}.
 */
export interface WorkerConnectOptions {
    /**
     * Slow-UI policy. Defaults to `drop-progress`, matching Facade Agent.
     *
     * - `drop-progress`: drop transient batches when the outbound queue is full;
     *   durable and terminal batches wait up to {@link durableTimeoutMs}.
     * - `disconnect`: close the subscription when the queue is full.
     * - `block-bounded`: wait up to {@link blockTimeoutMs} for an ack, then
     *   disconnect.
     */
    lagPolicy?: LagPolicy;
    /** Unacked outbound batch bound. Integer in `1..=1024`; defaults to 32. */
    queueCapacity?: number;
    /** Maximum `block-bounded` wait in milliseconds. Integer in `1..=60000`. */
    blockTimeoutMs?: number;
    /** Maximum durable-batch wait in milliseconds. Integer in `1..=60000`. */
    durableTimeoutMs?: number;
}
/**
 * Observable worker-to-UI queue pressure.
 */
export interface WorkerBackpressure {
    /** Batches sent and not yet acked. */
    queuedBatches: number;
    /** Transient events dropped since the last delivered batch. */
    droppedProgress: number;
    /** Active lag policy. */
    policy: LagPolicy;
}
interface RunState {
    agentId: string;
    runId?: string;
    session?: SessionSnapshot;
    inbox: WorkerEventBatch[];
    waiters: Array<() => void>;
    result?: RunResultSnapshot;
    error?: FinstackError;
    closed: boolean;
    iterating: boolean;
    backpressure: WorkerBackpressure;
    started: Promise<SessionSnapshot>;
    resolveStarted: (session: SessionSnapshot) => void;
    rejectStarted: (error: FinstackError) => void;
    finished: Promise<RunResultSnapshot>;
    resolveFinished: (snapshot: RunResultSnapshot) => void;
    rejectFinished: (error: FinstackError) => void;
}
/**
 * Snapshot facade reconstructed from a transferred event-batch buffer.
 *
 * This is not a wasm handle. Serialization already happened in the worker.
 */
export declare class WorkerEventBatch {
    #private;
    readonly firstSequence: number;
    readonly lastSequence: number;
    readonly droppedProgress: number;
    constructor(bytes: ArrayBuffer, firstSequence: number, lastSequence: number, droppedProgress: number);
    /**
     * Expand the transferred snapshot into individual events.
     *
     * @returns Event facades for this transport batch.
     * @example
     * ```ts
     * const events = batch.events();
     * ```
     */
    events(): WorkerEvent[];
    /**
     * Return the transferred JSON text.
     *
     * @returns Canonical JSON text of every contained event.
     */
    toJson(): string;
    /**
     * Copy the transferred JSON into bytes.
     *
     * @returns UTF-8 JSON for every event in this transport batch.
     */
    toJsonBytes(): Uint8Array;
}
/**
 * Snapshot facade for one event inside a transferred batch.
 */
export declare class WorkerEvent {
    #private;
    constructor(value: unknown);
    /** Event kind name such as `run_completed`. */
    get kind(): string;
    /** Durable or transient class inferred from the snapshot. */
    get eventClass(): string;
    /** Transient sequence. */
    get transientSequence(): number;
    /** Durable sequence, when the event is durable-derived. */
    get durableSequence(): number | undefined;
    /**
     * Return the transferred event JSON.
     *
     * @returns Canonical JSON text.
     */
    toJson(): string;
}
/**
 * Main-thread proxy for one worker-hosted Agent.
 */
export declare class WorkerAgent {
    #private;
    constructor(client: WorkerClient, agentId: string);
    /**
     * Start one run and return its proxy handle immediately.
     *
     * Dropping the returned {@link WorkerRun} detaches observation and does not
     * cancel the durable run. Session fields are available after the worker
     * accepts the start.
     *
     * @param input - Plain-text user input.
     * @param options - Optional timeout, cycle, and retry bounds.
     * @returns A worker-run proxy.
     * @example
     * ```ts
     * const run = agent.start("hello");
     * const result = await run.result();
     * ```
     */
    start(input: string, options?: RunOptions): WorkerRun;
    /**
     * Execute one run and await its committed result snapshot.
     *
     * @param input - Plain-text user input.
     * @param options - Optional timeout, cycle, and retry bounds.
     * @returns The retained terminal snapshot.
     * @throws {FinstackError} When the run fails, times out, or is cancelled.
     * @example
     * ```ts
     * const result = await agent.run("hello");
     * ```
     */
    run(input: string, options?: RunOptions): Promise<RunResultSnapshot>;
}
/**
 * Main-thread observation proxy for one worker-hosted run.
 *
 * Drop detaches observation and does not cancel execution.
 */
export declare class WorkerRun {
    #private;
    constructor(client: WorkerClient, state: RunState);
    /**
     * Session locator captured when the worker accepted the run.
     *
     * @throws {FinstackError} When start has not been acknowledged yet.
     */
    get session(): SessionSnapshot;
    /**
     * Batch-first asynchronous iterator over transferred snapshots.
     *
     * {@link EventOptions.signal} cancels iteration and closes delivery only.
     *
     * @param options - Optional abort signal for the iterator.
     * @returns An async iterable of {@link WorkerEventBatch} values.
     * @example
     * ```ts
     * for await (const batch of run.events()) {
     *   const first = batch.firstSequence;
     * }
     * ```
     */
    events(options?: EventOptions): AsyncIterable<WorkerEventBatch>;
    /**
     * Wait for the retained terminal snapshot.
     *
     * @returns The committed result snapshot.
     * @throws {FinstackError} When the run fails, times out, or is cancelled.
     * @example
     * ```ts
     * const result = await run.result();
     * ```
     */
    result(): Promise<RunResultSnapshot>;
    /** Read the worker-hosted run's latest confirmed state. */
    liveState(): Promise<RunStateSnapshot>;
    /** Wait until the worker-hosted latest-only view advances. */
    waitForLiveState(revision: number): Promise<RunStateSnapshot>;
    /**
     * Submit idempotent durable cancellation.
     *
     * @returns A promise that settles when cancellation is accepted.
     * @example
     * ```ts
     * await run.cancel();
     * await run.cancel();
     * ```
     */
    cancel(): Promise<void>;
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
    /**
     * Read the latest worker-to-UI queue pressure.
     *
     * @returns Queued batch count, dropped progress, and active policy.
     * @example
     * ```ts
     * const pressure = run.backpressure();
     * ```
     */
    backpressure(): WorkerBackpressure;
}
/**
 * Main-thread client for one Dedicated Worker that hosts WASM Agents.
 *
 * This object does not call `init()` and does not load the wasm module.
 */
export declare class WorkerClient {
    #private;
    constructor(worker: Worker, policy: LagPolicy);
    /** Allocate a unique protocol request id. */
    nextId(): string;
    /**
     * Construct one Agent inside the worker.
     *
     * @param options - Serializable factory payload. Do not pass JS handles.
     * @returns A main-thread Agent proxy.
     * @throws {FinstackError} When the worker rejects construction.
     * @example
     * ```ts
     * const agent = await client.create({ scenario: "model-only" });
     * ```
     */
    /**
     * Inspect one stored session inside the worker.
     *
     * The store stays in the worker. This does not continue an interrupted run.
     *
     * @param sessionId - Session identity to inspect.
     * @returns A provisional inspect snapshot.
     * @throws {FinstackError} When the worker rejects inspect.
     * @example
     * ```ts
     * const snapshot = await client.inspectSession(sessionId);
     * ```
     */
    inspectSession(sessionId: string): Promise<SessionInspectSnapshot>;
    create(options?: unknown): Promise<WorkerAgent>;
    /**
     * Cancel in-flight runs and terminate the Dedicated Worker.
     *
     * @returns A promise that settles after termination is requested.
     * @example
     * ```ts
     * await client.terminate();
     * ```
     */
    terminate(): Promise<void>;
    startRun(agentId: string, input: string, options?: RunOptions): WorkerRun;
    ack(state: RunState, lastSequence: number): void;
    closeEvents(state: RunState): Promise<void>;
    request(message: MainToWorker): Promise<WorkerToMain>;
}
/**
 * Connect a Dedicated Worker that already called {@link exposeWorkerHost}.
 *
 * The UI thread does not load wasm. Adapters stay inside the worker.
 *
 * @param worker - Module worker running the host helper.
 * @param options - Lag policy and queue bounds.
 * @returns A connected client.
 * @throws {FinstackError} When the handshake fails.
 * @example
 * ```ts
 * const worker = new Worker(new URL("./agent-worker.js", import.meta.url), {
 *   type: "module",
 * });
 * const client = await connectWorker(worker);
 * ```
 */
export declare function connectWorker(worker: Worker, options?: WorkerConnectOptions): Promise<WorkerClient>;
export {};
//# sourceMappingURL=worker-client.d.ts.map