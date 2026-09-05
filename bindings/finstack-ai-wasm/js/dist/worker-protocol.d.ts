import type { RunStateSnapshot, SessionInspectSnapshot } from "./agent.js";
import type { RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
/** Protocol version carried on every worker message. */
export declare const PROTOCOL_VERSION = 1;
/** Maximum JSON control-envelope bytes (Threat Model §8.3). */
export declare const MAX_CONTROL_BYTES: number;
/** Maximum transferred event-batch bytes (Threat Model §8.3). */
export declare const MAX_TRANSFER_BYTES: number;
/** Maximum transferred message-bearing live-state snapshot bytes. */
export declare const MAX_LIVE_STATE_BYTES: number;
/** Slow-consumer policy for the worker-to-UI queue. */
export type LagPolicy = "drop-progress" | "disconnect" | "block-bounded";
/** Main-thread to worker commands. */
export type MainToWorker = {
    v: 1;
    type: "init";
    id: string;
    lagPolicy?: LagPolicy;
    queueCapacity?: number;
    blockTimeoutMs?: number;
    durableTimeoutMs?: number;
} | {
    v: 1;
    type: "create";
    id: string;
    options?: unknown;
} | {
    v: 1;
    type: "start";
    id: string;
    agentId: string;
    input: string;
    options?: RunOptions;
} | {
    v: 1;
    type: "cancel";
    id: string;
    agentId: string;
    runId: string;
} | {
    v: 1;
    type: "liveState";
    id: string;
    agentId: string;
    runId: string;
} | {
    v: 1;
    type: "waitForLiveState";
    id: string;
    agentId: string;
    runId: string;
    revision: number;
} | {
    v: 1;
    type: "closeEvents";
    id: string;
    agentId: string;
    runId: string;
} | {
    v: 1;
    type: "ack";
    agentId: string;
    runId: string;
    lastSequence: number;
} | {
    v: 1;
    type: "inspect";
    id: string;
    sessionId: string;
} | {
    v: 1;
    type: "shutdown";
    id: string;
};
/** Worker to main-thread notifications. */
export type WorkerToMain = {
    v: 1;
    type: "ready";
    id: string;
} | {
    v: 1;
    type: "created";
    id: string;
    agentId: string;
} | {
    v: 1;
    type: "started";
    id: string;
    agentId: string;
    runId: string;
    session: SessionSnapshot;
} | {
    v: 1;
    type: "batch";
    agentId: string;
    runId: string;
    firstSequence: number;
    lastSequence: number;
    droppedProgress: number;
    durable: boolean;
} | {
    v: 1;
    type: "result";
    id?: string;
    agentId: string;
    runId: string;
    snapshot: RunResultSnapshot;
} | {
    v: 1;
    type: "error";
    id?: string;
    agentId?: string;
    runId?: string;
    code: string;
    retryable: boolean;
    message: string;
    context?: SessionSnapshot;
} | {
    v: 1;
    type: "backpressure";
    agentId: string;
    runId: string;
    queuedBatches: number;
    droppedProgress: number;
    policy: LagPolicy;
} | {
    v: 1;
    type: "terminated";
    reason: string;
} | {
    v: 1;
    type: "inspected";
    id: string;
    snapshot: SessionInspectSnapshot;
} | {
    v: 1;
    type: "state";
    id: string;
    snapshot?: RunStateSnapshot;
};
/**
 * Encode a control envelope and reject oversized payloads.
 *
 * @param message - Protocol object.
 * @returns UTF-8 JSON text.
 * @throws {FinstackError} When the envelope exceeds {@link MAX_CONTROL_BYTES}.
 */
export declare function encodeControl(message: MainToWorker | WorkerToMain): string;
/**
 * Decode a control envelope and require version, type, and handle correlation.
 *
 * @param data - `postMessage` payload.
 * @returns A typed protocol message.
 * @throws {FinstackError} When the payload is oversized, unversioned, or uncorrelated.
 */
export declare function decodeMainToWorker(data: unknown): MainToWorker;
/**
 * Decode a worker-to-main envelope.
 *
 * @param data - `postMessage` payload.
 * @returns A typed protocol message.
 * @throws {FinstackError} When the payload is oversized, unversioned, or uncorrelated.
 */
export declare function decodeWorkerToMain(data: unknown): WorkerToMain;
/**
 * Require a transferred batch buffer to stay inside the size bound.
 *
 * @param bytes - Transferred event-batch payload.
 * @returns The validated transferred buffer.
 * @throws {FinstackError} When the buffer is missing or too large.
 */
export declare function requireTransfer(bytes: unknown): ArrayBuffer;
//# sourceMappingURL=worker-protocol.d.ts.map