import type { RunStateSnapshot, SessionInspectSnapshot } from "./agent.js";
import { FinstackError, sessionSnapshot } from "./errors.js";
import type { RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";

/** Protocol version carried on every worker message. */
export const PROTOCOL_VERSION = 1;

/** Maximum JSON control-envelope bytes (Threat Model §8.3). */
export const MAX_CONTROL_BYTES = 16 * 1024;

/** Maximum transferred event-batch bytes (Threat Model §8.3). */
export const MAX_TRANSFER_BYTES = 256 * 1024;

/** Maximum transferred message-bearing live-state snapshot bytes. */
export const MAX_LIVE_STATE_BYTES = 16 * 1024 * 1024;

const MAX_WORKER_QUEUE_CAPACITY = 1024;
const MAX_WORKER_TIMEOUT_MS = 60_000;

/** Slow-consumer policy for the worker-to-UI queue. */
export type LagPolicy = "drop-progress" | "disconnect" | "block-bounded";

/** Main-thread to worker commands. */
export type MainToWorker =
  | {
      v: 1;
      type: "init";
      id: string;
      lagPolicy?: LagPolicy;
      queueCapacity?: number;
      blockTimeoutMs?: number;
      durableTimeoutMs?: number;
    }
  | { v: 1; type: "create"; id: string; options?: unknown }
  | {
      v: 1;
      type: "start";
      id: string;
      agentId: string;
      input: string;
      options?: RunOptions;
    }
  | {
      v: 1;
      type: "cancel";
      id: string;
      agentId: string;
      runId: string;
    }
  | { v: 1; type: "liveState"; id: string; agentId: string; runId: string }
  | {
      v: 1;
      type: "waitForLiveState";
      id: string;
      agentId: string;
      runId: string;
      revision: number;
    }
  | { v: 1; type: "closeEvents"; id: string; agentId: string; runId: string }
  | { v: 1; type: "ack"; agentId: string; runId: string; lastSequence: number }
  | { v: 1; type: "inspect"; id: string; sessionId: string }
  | { v: 1; type: "shutdown"; id: string };

/** Worker to main-thread notifications. */
export type WorkerToMain =
  | { v: 1; type: "ready"; id: string }
  | { v: 1; type: "created"; id: string; agentId: string }
  | {
      v: 1;
      type: "started";
      id: string;
      agentId: string;
      runId: string;
      session: SessionSnapshot;
    }
  | {
      v: 1;
      type: "batch";
      agentId: string;
      runId: string;
      firstSequence: number;
      lastSequence: number;
      droppedProgress: number;
      durable: boolean;
    }
  | {
      v: 1;
      type: "result";
      id?: string;
      agentId: string;
      runId: string;
      snapshot: RunResultSnapshot;
    }
  | {
      v: 1;
      type: "error";
      id?: string;
      agentId?: string;
      runId?: string;
      code: string;
      retryable: boolean;
      message: string;
      context?: SessionSnapshot;
    }
  | {
      v: 1;
      type: "backpressure";
      agentId: string;
      runId: string;
      queuedBatches: number;
      droppedProgress: number;
      policy: LagPolicy;
    }
  | { v: 1; type: "terminated"; reason: string }
  | { v: 1; type: "inspected"; id: string; snapshot: SessionInspectSnapshot }
  | { v: 1; type: "state"; id: string; snapshot?: RunStateSnapshot };

/**
 * Encode a control envelope and reject oversized payloads.
 *
 * @param message - Protocol object.
 * @returns UTF-8 JSON text.
 * @throws {FinstackError} When the envelope exceeds {@link MAX_CONTROL_BYTES}.
 */
export function encodeControl(message: MainToWorker | WorkerToMain): string {
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
export function decodeMainToWorker(data: unknown): MainToWorker {
  const record = decodeRecord(data);
  const type = requiredString(record, "type");
  switch (type) {
    case "shutdown":
      return { v: 1, type, id: requiredString(record, "id") };
    case "init": {
      const message: MainToWorker = { v: 1, type, id: requiredString(record, "id") };
      if (
        record.lagPolicy === "drop-progress" ||
        record.lagPolicy === "disconnect" ||
        record.lagPolicy === "block-bounded"
      ) {
        message.lagPolicy = record.lagPolicy;
      }
      if (record.queueCapacity !== undefined) {
        message.queueCapacity = requiredBoundedInteger(
          record,
          "queueCapacity",
          1,
          MAX_WORKER_QUEUE_CAPACITY,
        );
      }
      if (record.blockTimeoutMs !== undefined) {
        message.blockTimeoutMs = requiredBoundedInteger(
          record,
          "blockTimeoutMs",
          1,
          MAX_WORKER_TIMEOUT_MS,
        );
      }
      if (record.durableTimeoutMs !== undefined) {
        message.durableTimeoutMs = requiredBoundedInteger(
          record,
          "durableTimeoutMs",
          1,
          MAX_WORKER_TIMEOUT_MS,
        );
      }
      return message;
    }
    case "create": {
      const message: MainToWorker = { v: 1, type, id: requiredString(record, "id") };
      if (record.options !== undefined) {
        message.options = record.options;
      }
      return message;
    }
    case "start": {
      const message: MainToWorker = {
        v: 1,
        type,
        id: requiredString(record, "id"),
        agentId: requiredString(record, "agentId"),
        input: requiredString(record, "input"),
      };
      if (record.options !== undefined && typeof record.options === "object") {
        message.options = record.options as RunOptions;
      }
      return message;
    }
    case "cancel":
      return {
        v: 1,
        type,
        id: requiredString(record, "id"),
        agentId: requiredString(record, "agentId"),
        runId: requiredString(record, "runId"),
      };
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
        revision: requiredSafeInteger(record, "revision"),
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
        lastSequence: requiredSafeInteger(record, "lastSequence"),
      };
    case "inspect":
      return {
        v: 1,
        type,
        id: requiredString(record, "id"),
        sessionId: requiredString(record, "sessionId"),
      };
    default: {
      const _exhaustive: never = type as never;
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
export function decodeWorkerToMain(data: unknown): WorkerToMain {
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
        firstSequence: requiredSafeInteger(record, "firstSequence"),
        lastSequence: requiredSafeInteger(record, "lastSequence"),
        droppedProgress: requiredSafeInteger(record, "droppedProgress"),
        durable: record.durable === true,
      };
    case "result": {
      const message: WorkerToMain = {
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
      const message: WorkerToMain = {
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
        queuedBatches: requiredSafeInteger(record, "queuedBatches"),
        droppedProgress: requiredSafeInteger(record, "droppedProgress"),
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
      const _exhaustive: never = type as never;
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
export function requireTransfer(bytes: unknown): ArrayBuffer {
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

function decodeRecord(data: unknown): Record<string, unknown> {
  if (typeof data === "string") {
    if (data.length > MAX_CONTROL_BYTES) {
      throw new FinstackError("worker message exceeds size bound", {
        code: "agent_run_invalid_configuration",
        retryable: false,
      });
    }
    data = JSON.parse(data) as unknown;
  }
  if (data === null || typeof data !== "object") {
    throw new FinstackError("worker message is not an object", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  const record = data as Record<string, unknown>;
  if (record.v !== PROTOCOL_VERSION) {
    throw new FinstackError("unsupported worker protocol version", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  return record;
}

function requiredString(record: Record<string, unknown>, key: string): string {
  const value = record[key];
  if (typeof value !== "string" || value.length === 0) {
    throw new FinstackError(`worker message missing ${key}`, {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  return value;
}

function requiredSafeInteger(record: Record<string, unknown>, key: string): number {
  return requiredBoundedInteger(record, key, 0, Number.MAX_SAFE_INTEGER);
}

function requiredBoundedInteger(
  record: Record<string, unknown>,
  key: string,
  minimum: number,
  maximum: number,
): number {
  const value = record[key];
  if (
    typeof value !== "number" ||
    !Number.isSafeInteger(value) ||
    value < minimum ||
    value > maximum
  ) {
    throw new FinstackError(`worker message has invalid ${key}`, {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  return value;
}

function requiredSession(value: unknown): SessionSnapshot {
  const snapshot = sessionSnapshot(value);
  if (snapshot === undefined) {
    throw new FinstackError("worker message missing session", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  return snapshot;
}

function requiredResult(value: unknown): RunResultSnapshot {
  const session = requiredSession(value);
  if (value === null || typeof value !== "object") {
    throw new FinstackError("worker message missing result", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  const text = (value as { text?: unknown }).text;
  if (typeof text !== "string") {
    throw new FinstackError("worker message missing result text", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  const output = "output" in value ? value.output : null;
  return { ...session, text, output };
}

function requiredInspect(value: unknown): SessionInspectSnapshot {
  if (value === null || typeof value !== "object") {
    throw new FinstackError("worker message missing inspect snapshot", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  const record = value as Record<string, unknown>;
  const phase = record.phase;
  const headSequence = record.headSequence;
  if (
    typeof record.sessionId !== "string" ||
    typeof headSequence !== "number" ||
    !Number.isSafeInteger(headSequence) ||
    headSequence < 0 ||
    (phase !== "empty" &&
      phase !== "in_progress" &&
      phase !== "completed" &&
      phase !== "failed" &&
      phase !== "cancelled")
  ) {
    throw new FinstackError("worker message missing inspect snapshot", {
      code: "agent_run_invalid_configuration",
      retryable: false,
    });
  }
  const snapshot: SessionInspectSnapshot = {
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

function requiredPolicy(value: unknown): LagPolicy {
  if (value === "drop-progress" || value === "disconnect" || value === "block-bounded") {
    return value;
  }
  throw new FinstackError("worker message missing lag policy", {
    code: "agent_run_invalid_configuration",
    retryable: false,
  });
}
