/**
 * Production browser topology: Dedicated Worker hosts the WASM engine.
 *
 * Main-thread {@link Agent.create} remains the documented host-compatible
 * mode. This module does not load wasm-bindgen glue on the UI thread.
 *
 * Threaded WASM and SharedArrayBuffer remain a post-preview opt-in that
 * requires cross-origin isolation and a new ADR-031 reconsideration.
 */
export { FinstackError } from "./errors.js";
export { WorkerAgent, WorkerClient, WorkerEvent, WorkerEventBatch, WorkerRun, connectWorker, } from "./worker-client.js";
export { exposeWorkerHost } from "./worker-host.js";
//# sourceMappingURL=worker.js.map