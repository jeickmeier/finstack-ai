import { FinstackError, sessionSnapshot } from "./errors.js";
/** Protocol version carried on every worker message. */
export const PROTOCOL_VERSION = 1;
/** Maximum JSON control-envelope bytes (Threat Model §8.3). */
export const MAX_CONTROL_BYTES = 16 * 1024;
/** Maximum transferred event-batch bytes (Threat Model §8.3). */
export const MAX_TRANSFER_BYTES = 256 * 1024;
/** Maximum transferred message-bearing live-state snapshot bytes. */
export const MAX_LIVE_STATE_BYTES = 16 * 1024 * 1024;
/**
 * Encode a control envelope and reject oversized payloads.
 *
 * @param message - Protocol object.
 * @returns UTF-8 JSON text.
 * @throws {FinstackError} When the envelope exceeds {@link MAX_CONTROL_BYTES}.
 */
export function encodeControl(message) {
    const encoded = JSON.stringify(message);
    if (encoded.length > MAX_CONTROL_BYTES) {
        throw new FinstackError("worker message exceeds size bound", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return encoded;
}
/**
 * Decode a control envelope and require version, type, and handle correlation.
 *
 * @param data - `postMessage` payload.
 * @returns A typed protocol message.
 * @throws {FinstackError} When the payload is oversized, unversioned, or uncorrelated.
 */
export function decodeMainToWorker(data) {
    const record = decodeRecord(data);
    const type = requiredString(record, "type");
    switch (type) {
        case "shutdown":
            return { v: 1, type, id: requiredString(record, "id") };
        case "init": {
            const message = { v: 1, type, id: requiredString(record, "id") };
            if (record.lagPolicy === "drop-progress" ||
                record.lagPolicy === "disconnect" ||
                record.lagPolicy === "block-bounded") {
                message.lagPolicy = record.lagPolicy;
            }
            if (typeof record.queueCapacity === "number") {
                message.queueCapacity = record.queueCapacity;
            }
            if (typeof record.blockTimeoutMs === "number") {
                message.blockTimeoutMs = record.blockTimeoutMs;
            }
            if (typeof record.durableTimeoutMs === "number") {
                message.durableTimeoutMs = record.durableTimeoutMs;
            }
            return message;
        }
        case "create": {
            const message = { v: 1, type, id: requiredString(record, "id") };
            if (record.options !== undefined) {
                message.options = record.options;
            }
            return message;
        }
        case "start": {
            const message = {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                input: requiredString(record, "input"),
            };
            if (record.options !== undefined && typeof record.options === "object") {
                message.options = record.options;
            }
            return message;
        }
        case "cancel": {
            const message = {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
            };
            if (typeof record.reason === "string") {
                message.reason = record.reason;
            }
            return message;
        }
        case "liveState":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
            };
        case "waitForLiveState":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                revision: requiredNumber(record, "revision"),
            };
        case "closeEvents":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
            };
        case "ack":
            return {
                v: 1,
                type,
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                lastSequence: requiredNumber(record, "lastSequence"),
            };
        case "inspect":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                sessionId: requiredString(record, "sessionId"),
            };
        default: {
            const _exhaustive = type;
            throw new FinstackError(`unsupported worker command: ${String(_exhaustive)}`, {
                code: "agent_run_invalid_configuration",
                retryable: false,
            });
        }
    }
}
/**
 * Decode a worker-to-main envelope.
 *
 * @param data - `postMessage` payload.
 * @returns A typed protocol message.
 * @throws {FinstackError} When the payload is oversized, unversioned, or uncorrelated.
 */
export function decodeWorkerToMain(data) {
    const record = decodeRecord(data);
    const type = requiredString(record, "type");
    switch (type) {
        case "ready":
            return { v: 1, type, id: requiredString(record, "id") };
        case "created":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
            };
        case "started":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                session: requiredSession(record.session),
            };
        case "batch":
            return {
                v: 1,
                type,
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                firstSequence: requiredNumber(record, "firstSequence"),
                lastSequence: requiredNumber(record, "lastSequence"),
                droppedProgress: requiredNumber(record, "droppedProgress"),
                durable: record.durable === true,
            };
        case "result": {
            const message = {
                v: 1,
                type,
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                snapshot: requiredResult(record.snapshot),
            };
            if (typeof record.id === "string") {
                message.id = record.id;
            }
            return message;
        }
        case "error": {
            const message = {
                v: 1,
                type,
                code: requiredString(record, "code"),
                retryable: record.retryable === true,
                message: requiredString(record, "message"),
            };
            if (typeof record.id === "string") {
                message.id = record.id;
            }
            if (typeof record.agentId === "string") {
                message.agentId = record.agentId;
            }
            if (typeof record.runId === "string") {
                message.runId = record.runId;
            }
            const context = sessionSnapshot(record.context);
            if (context !== undefined) {
                message.context = context;
            }
            return message;
        }
        case "backpressure":
            return {
                v: 1,
                type,
                agentId: requiredString(record, "agentId"),
                runId: requiredString(record, "runId"),
                queuedBatches: requiredNumber(record, "queuedBatches"),
                droppedProgress: requiredNumber(record, "droppedProgress"),
                policy: requiredPolicy(record.policy),
            };
        case "terminated":
            return { v: 1, type, reason: requiredString(record, "reason") };
        case "inspected":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
                snapshot: requiredInspect(record.snapshot),
            };
        case "state":
            return {
                v: 1,
                type,
                id: requiredString(record, "id"),
            };
        default: {
            const _exhaustive = type;
            throw new FinstackError(`unsupported worker event: ${String(_exhaustive)}`, {
                code: "agent_run_invalid_configuration",
                retryable: false,
            });
        }
    }
}
/**
 * Require a transferred batch buffer to stay inside the size bound.
 *
 * @param bytes - Transferred event-batch payload.
 * @throws {FinstackError} When the buffer is missing or too large.
 */
export function requireTransfer(bytes) {
    if (!(bytes instanceof ArrayBuffer)) {
        throw new FinstackError("worker batch transfer is missing", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    if (bytes.byteLength > MAX_TRANSFER_BYTES) {
        throw new FinstackError("worker batch transfer exceeds size bound", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return bytes;
}
function decodeRecord(data) {
    if (typeof data === "string") {
        if (data.length > MAX_CONTROL_BYTES) {
            throw new FinstackError("worker message exceeds size bound", {
                code: "agent_run_invalid_configuration",
                retryable: false,
            });
        }
        data = JSON.parse(data);
    }
    if (data === null || typeof data !== "object") {
        throw new FinstackError("worker message is not an object", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    const record = data;
    if (record.v !== PROTOCOL_VERSION) {
        throw new FinstackError("unsupported worker protocol version", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return record;
}
function requiredString(record, key) {
    const value = record[key];
    if (typeof value !== "string" || value.length === 0) {
        throw new FinstackError(`worker message missing ${key}`, {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return value;
}
function requiredNumber(record, key) {
    const value = record[key];
    if (typeof value !== "number" || !Number.isFinite(value)) {
        throw new FinstackError(`worker message missing ${key}`, {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return value;
}
function requiredSession(value) {
    const snapshot = sessionSnapshot(value);
    if (snapshot === undefined) {
        throw new FinstackError("worker message missing session", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return snapshot;
}
function requiredResult(value) {
    const session = requiredSession(value);
    if (value === null || typeof value !== "object") {
        throw new FinstackError("worker message missing result", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    const text = value.text;
    if (typeof text !== "string") {
        throw new FinstackError("worker message missing result text", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    return { ...session, text };
}
function requiredInspect(value) {
    if (value === null || typeof value !== "object") {
        throw new FinstackError("worker message missing inspect snapshot", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    const record = value;
    const phase = record.phase;
    const headSequence = Number(record.headSequence);
    if (typeof record.sessionId !== "string" ||
        !Number.isFinite(headSequence) ||
        (phase !== "empty" &&
            phase !== "in_progress" &&
            phase !== "completed" &&
            phase !== "failed" &&
            phase !== "cancelled")) {
        throw new FinstackError("worker message missing inspect snapshot", {
            code: "agent_run_invalid_configuration",
            retryable: false,
        });
    }
    const snapshot = {
        sessionId: record.sessionId,
        headSequence,
        phase,
    };
    if (typeof record.resultText === "string") {
        snapshot.resultText = record.resultText;
    }
    if (typeof record.lastRecordKind === "string") {
        snapshot.lastRecordKind = record.lastRecordKind;
    }
    return snapshot;
}
function requiredPolicy(value) {
    if (value === "drop-progress" || value === "disconnect" || value === "block-bounded") {
        return value;
    }
    throw new FinstackError("worker message missing lag policy", {
        code: "agent_run_invalid_configuration",
        retryable: false,
    });
}
//# sourceMappingURL=worker-protocol.js.map