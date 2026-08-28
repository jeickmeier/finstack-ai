import type { RunStateSnapshot, SessionInspectSnapshot } from "./agent.js";
import { FinstackError } from "./errors.js";
import type { EventOptions, RunOptions, RunResultSnapshot, SessionSnapshot } from "./errors.js";
import {
  encodeControl,
  decodeMainToWorker,
  type LagPolicy,
  type WorkerToMain,
} from "./worker-protocol.js";

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
  readonly locator: { toDict(): SessionSnapshot };
  events(options?: EventOptions): AsyncIterable<WorkerHostEventBatch>;
  result(): Promise<WorkerHostRunResult>;
  liveState(): Promise<RunStateSnapshot>;
  waitForLiveState(revision: number): Promise<RunStateSnapshot>;
  cancel(): Promise<void>;
  closeEvents(): Promise<void>;
}

/** Structural terminal result used inside the worker. */
export interface WorkerHostRunResult {
  readonly text: string;
  readonly output: unknown | null;
  toDict(): RunResultSnapshot;
  readonly session: { toDict(): SessionSnapshot };
}

/** Structural event batch used inside the worker. */
export interface WorkerHostEventBatch {
  readonly firstSequence: number;
  readonly lastSequence: number;
  readonly droppedProgress: number;
  events(): Array<{ readonly eventClass: string }>;
  toJsonBytes(): Uint8Array;
}

/**
 * Factory that constructs one Agent inside the Dedicated Worker.
 */
export interface WorkerHostFactory {
  create(options?: unknown): Promise<WorkerHostAgent>;
  inspectSession?(sessionId: string): Promise<SessionInspectSnapshot>;
}

interface WorkerScope {
  postMessage: (message: unknown, transfer?: Transferable[]) => void;
  addEventListener: (type: "message", listener: (event: MessageEvent) => void) => void;
}

/**
 * Worker-to-main `postMessage` backpressure. In-process event lag is owned by
 * the Rust `EventHub`; this slot only bounds the dedicated-worker channel.
 */
interface RunSlot {
  agentId: string;
  runId: string;
  run: WorkerHostRun;
  session: SessionSnapshot;
  policy: LagPolicy;
  queueCapacity: number;
  blockTimeoutMs: number;
  durableTimeoutMs: number;
  outstandingSequences: number[];
  dropped: number;
  waiters: Array<() => void>;
  closed: boolean;
}

/**
 * Serve the worker protocol from a Dedicated Worker.
 *
 * Call this from the worker module after constructing adapters locally.
 * The UI thread must not pass `JsModel` or other host objects across the
 * boundary.
 *
 * @param factory - Creates one Agent from a serializable options payload.
 * @returns Nothing; the function installs the worker message handler.
 * @example
 * ```ts
 * exposeWorkerHost({
 *   async create() {
 *     return Agent.create({ model });
 *   },
 * });
 * ```
 */
export function exposeWorkerHost(factory: WorkerHostFactory): void {
  const scope = globalThis as unknown as WorkerScope;
  const agents = new Map<string, WorkerHostAgent>();
  const runs = new Map<string, RunSlot>();
  let nextAgent = 0;
  let defaults = {
    policy: "drop-progress" as LagPolicy,
    queueCapacity: 32,
    blockTimeoutMs: 2000,
    durableTimeoutMs: 2000,
  };

  scope.addEventListener("message", (event) => {
    void handle(event.data);
  });

  async function handle(data: unknown): Promise<void> {
    let requestId: string | undefined;
    if (
      data !== null &&
      typeof data === "object" &&
      "id" in data &&
      typeof data.id === "string"
    ) {
      requestId = data.id;
    }
    try {
      await dispatch(data);
    } catch (error) {
      postError(undefined, FinstackError.fromUnknown(error), requestId);
    }
  }

  async function dispatch(data: unknown): Promise<void> {
    const message = decodeMainToWorker(data);
    switch (message.type) {
      case "init":
        if (message.lagPolicy !== undefined) {
          defaults.policy = message.lagPolicy;
        }
        if (message.queueCapacity !== undefined) {
          defaults.queueCapacity = message.queueCapacity;
        }
        if (message.blockTimeoutMs !== undefined) {
          defaults.blockTimeoutMs = message.blockTimeoutMs;
        }
        if (message.durableTimeoutMs !== undefined) {
          defaults.durableTimeoutMs = message.durableTimeoutMs;
        }
        post({ v: 1, type: "ready", id: message.id });
        return;
      case "create": {
        const agent = await factory.create(message.options);
        const agentId = `js.agent.${nextAgent}`;
        nextAgent += 1;
        agents.set(agentId, agent);
        post({ v: 1, type: "created", id: message.id, agentId });
        return;
      }
      case "start": {
        const agent = agents.get(message.agentId);
        if (agent === undefined) {
          throw new FinstackError("unknown worker agent", {
            code: "agent_run_invalid_configuration",
            retryable: false,
          });
        }
        const run = agent.start(message.input, message.options);
        const session = run.locator.toDict();
        const slot: RunSlot = {
          agentId: message.agentId,
          runId: session.runId,
          run,
          session,
          policy: defaults.policy,
          queueCapacity: defaults.queueCapacity,
          blockTimeoutMs: defaults.blockTimeoutMs,
          durableTimeoutMs: defaults.durableTimeoutMs,
          outstandingSequences: [],
          dropped: 0,
          waiters: [],
          closed: false,
        };
        runs.set(session.runId, slot);
        post({
          v: 1,
          type: "started",
          id: message.id,
          agentId: message.agentId,
          runId: session.runId,
          session,
        });
        void pump(slot);
        return;
      }
      case "cancel": {
        const slot = requireRun(message.agentId, message.runId);
        await slot.run.cancel();
        post({ v: 1, type: "ready", id: message.id });
        return;
      }
      case "liveState": {
        const slot = requireRun(message.agentId, message.runId);
        postState(message.id, await slot.run.liveState());
        return;
      }
      case "waitForLiveState": {
        const slot = requireRun(message.agentId, message.runId);
        postState(message.id, await slot.run.waitForLiveState(message.revision));
        return;
      }
      case "closeEvents": {
        const slot = requireRun(message.agentId, message.runId);
        slot.closed = true;
        await slot.run.closeEvents();
        post({ v: 1, type: "ready", id: message.id });
        return;
      }
      case "ack": {
        const slot = requireRun(message.agentId, message.runId);
        if (slot.outstandingSequences[0] !== message.lastSequence) {
          return;
        }
        slot.outstandingSequences.shift();
        const waiter = slot.waiters.shift();
        waiter?.();
        return;
      }
      case "inspect": {
        if (factory.inspectSession === undefined) {
          throw new FinstackError("worker inspect is unavailable", {
            code: "agent_run_invalid_configuration",
            retryable: false,
          });
        }
        const snapshot = await factory.inspectSession(message.sessionId);
        post({
          v: 1,
          type: "inspected",
          id: message.id,
          snapshot: {
            ...snapshot,
            headSequence: Number(snapshot.headSequence),
          },
        });
        return;
      }
      case "shutdown":
        for (const slot of runs.values()) {
          slot.closed = true;
          await slot.run.cancel().catch(() => undefined);
        }
        post({ v: 1, type: "terminated", reason: "shutdown" });
        return;
      default: {
        const _exhaustive: never = message;
        throw new FinstackError(`unsupported worker command: ${String(_exhaustive)}`, {
          code: "agent_run_invalid_configuration",
          retryable: false,
        });
      }
    }
  }

  async function pump(slot: RunSlot): Promise<void> {
    try {
      for await (const batch of slot.run.events()) {
        if (slot.closed) {
          break;
        }
        const durable = batch
          .events()
          .some((event) => event.eventClass === "durable_derived");
        const accepted = await enqueue(slot, batch, durable);
        if (accepted === "disconnected") {
          postError(slot, new FinstackError("worker event subscription disconnected", {
            code: "agent_run_runtime_failure",
            retryable: false,
            context: slot.session,
          }));
          await slot.run.closeEvents();
          break;
        }
      }
      const result = await slot.run.result();
      post({
        v: 1,
        type: "result",
        agentId: slot.agentId,
        runId: slot.runId,
        snapshot: result.toDict(),
      });
    } catch (error) {
      postError(slot, FinstackError.fromUnknown(error));
    }
  }

  async function enqueue(
    slot: RunSlot,
    batch: WorkerHostEventBatch,
    durable: boolean,
  ): Promise<"sent" | "dropped" | "disconnected"> {
    const timeoutMs = durable ? slot.durableTimeoutMs : slot.blockTimeoutMs;
    const started = Date.now();
    while (slot.outstandingSequences.length >= slot.queueCapacity) {
      if (slot.policy === "disconnect") {
        return "disconnected";
      }
      if (slot.policy === "drop-progress" && !durable) {
        slot.dropped += Math.max(1, batch.lastSequence - batch.firstSequence + 1);
        publishBackpressure(slot);
        return "dropped";
      }
      const remaining = timeoutMs - (Date.now() - started);
      if (remaining <= 0) {
        if (durable) {
          break;
        }
        if (slot.policy === "drop-progress") {
          slot.dropped += Math.max(1, batch.lastSequence - batch.firstSequence + 1);
          publishBackpressure(slot);
          return "dropped";
        }
        return "disconnected";
      }
      await waitForAck(slot, remaining);
    }
    const bytes = copyBuffer(batch.toJsonBytes());
    const droppedProgress = batch.droppedProgress + slot.dropped;
    slot.dropped = 0;
    slot.outstandingSequences.push(batch.lastSequence);
    post(
      {
        v: 1,
        type: "batch",
        agentId: slot.agentId,
        runId: slot.runId,
        firstSequence: batch.firstSequence,
        lastSequence: batch.lastSequence,
        droppedProgress,
        durable,
      },
      [bytes],
    );
    publishBackpressure(slot);
    return "sent";
  }

  function waitForAck(slot: RunSlot, timeoutMs: number): Promise<void> {
    return new Promise((resolve) => {
      const timer = setTimeout(() => {
        const index = slot.waiters.indexOf(onAck);
        if (index >= 0) {
          slot.waiters.splice(index, 1);
        }
        resolve();
      }, timeoutMs);
      const onAck = (): void => {
        clearTimeout(timer);
        resolve();
      };
      slot.waiters.push(onAck);
    });
  }

  function publishBackpressure(slot: RunSlot): void {
    post({
      v: 1,
      type: "backpressure",
      agentId: slot.agentId,
      runId: slot.runId,
      queuedBatches: slot.outstandingSequences.length,
      droppedProgress: slot.dropped,
      policy: slot.policy,
    });
  }

  function requireRun(agentId: string, runId: string): RunSlot {
    const slot = runs.get(runId);
    if (slot === undefined || slot.agentId !== agentId) {
      throw new FinstackError("unknown worker run", {
        code: "agent_run_invalid_configuration",
        retryable: false,
      });
    }
    return slot;
  }

  function post(message: WorkerToMain, transfer?: Transferable[]): void {
    const encoded = JSON.parse(encodeControl(message)) as WorkerToMain;
    if (transfer !== undefined) {
      scope.postMessage({ envelope: encoded, bytes: transfer[0] }, transfer);
      return;
    }
    scope.postMessage(encoded);
  }

  function postState(id: string, snapshot: RunStateSnapshot): void {
    const bytes = new TextEncoder().encode(JSON.stringify(snapshot));
    post({ v: 1, type: "state", id }, [bytes.buffer]);
  }

  function postError(
    slot: RunSlot | undefined,
    error: FinstackError,
    requestId?: string,
  ): void {
    const message: WorkerToMain = {
      v: 1,
      type: "error",
      code: error.code,
      retryable: error.retryable,
      message: error.message,
    };
    if (requestId !== undefined) {
      message.id = requestId;
    }
    if (slot !== undefined) {
      message.agentId = slot.agentId;
      message.runId = slot.runId;
      message.context = slot.session;
    }
    if (error.context !== undefined && message.context === undefined) {
      message.context = error.context;
    }
    post(message);
  }
}

function copyBuffer(bytes: Uint8Array): ArrayBuffer {
  const copy = new ArrayBuffer(bytes.byteLength);
  new Uint8Array(copy).set(bytes);
  return copy;
}
