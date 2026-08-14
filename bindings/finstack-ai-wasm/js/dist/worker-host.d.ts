import type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
/**
 * Structural Agent surface used inside the worker. The factory supplies the
 * real {@link Agent} so this module does not load wasm-bindgen glue.
 */
export interface WorkerHostAgent {
    start(input: string, options?: RunOptions): WorkerHostRun;
    run(input: string, options?: RunOptions): Promise<WorkerHostRunResult>;
}
/** Structural run handle used inside the worker. */
export interface WorkerHostRun {
    readonly session: {
        toDict(): SessionSnapshot;
    };
    events(options?: EventOptions): AsyncIterable<WorkerHostEventBatch>;
    result(): Promise<WorkerHostRunResult>;
    cancel(reason?: string): Promise<void>;
    closeEvents(): Promise<void>;
}
/** Structural terminal result used inside the worker. */
export interface WorkerHostRunResult {
    readonly text: string;
    toDict(): RunResultSnapshot;
    readonly session: {
        toDict(): SessionSnapshot;
    };
}
/** Structural event batch used inside the worker. */
export interface WorkerHostEventBatch {
    readonly firstSequence: number;
    readonly lastSequence: number;
    readonly droppedProgress: number;
    events(): Array<{
        readonly eventClass: string;
    }>;
    toJsonBytes(): Uint8Array;
}
/**
 * Factory that constructs one Agent inside the Dedicated Worker.
 */
export interface WorkerHostFactory {
    create(options?: unknown): Promise<WorkerHostAgent>;
}
/**
 * Serve the worker protocol from a Dedicated Worker.
 *
 * Call this from the worker module after constructing adapters locally.
 * The UI thread must not pass `JsModel` or other host objects across the
 * boundary.
 *
 * @param factory - Creates one Agent from a serializable options payload.
 * @example
 * ```ts
 * exposeWorkerHost({
 *   async create() {
 *     return Agent.create({ model });
 *   },
 * });
 * ```
 */
export declare function exposeWorkerHost(factory: WorkerHostFactory): void;
//# sourceMappingURL=worker-host.d.ts.map