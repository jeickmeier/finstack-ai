import type { RunStateSnapshot, SessionInspectSnapshot } from "./agent.js";
import { FinstackError, sessionSnapshot } from "./errors.js";
import type {
  EventOptions,
  RunOptions,
  RunResultSnapshot,
  SessionSnapshot,
} from "./errors.js";
import {
  decodeWorkerToMain,
  encodeControl,
  MAX_LIVE_STATE_BYTES,
  requireTransfer,
  type LagPolicy,
  type MainToWorker,
  type WorkerToMain,
} from "./worker-protocol.js";

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

interface Pending {
  resolve: (message: WorkerToMain) => void;
  reject: (error: FinstackError) => void;
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
export class WorkerEventBatch {
  readonly #json: string;
  readonly #events: unknown[];
  readonly firstSequence: number;
  readonly lastSequence: number;
  readonly droppedProgress: number;

  constructor(
    bytes: ArrayBuffer,
    firstSequence: number,
    lastSequence: number,
    droppedProgress: number,
  ) {
    this.#json = new TextDecoder().decode(bytes);
    const parsed: unknown = JSON.parse(this.#json);
    if (!Array.isArray(parsed)) {
      throw new FinstackError("worker batch snapshot is not an array", {
        code: "agent_run_invalid_configuration",
        retryable: false,
      });
    }
    this.#events = parsed;
    this.firstSequence = firstSequence;
    this.lastSequence = lastSequence;
    this.droppedProgress = droppedProgress;
  }

  /**
   * Expand the transferred snapshot into individual events.
   *
   * @returns Event facades for this transport batch.
   * @example
   * ```ts
   * const events = batch.events();
   * ```
   */
  events(): WorkerEvent[] {
    return this.#events.map((event) => new WorkerEvent(event));
  }

  /**
   * Return the transferred JSON text.
   *
   * @returns Canonical JSON text of every contained event.
   */
  toJson(): string {
    return this.#json;
  }

  /**
   * Copy the transferred JSON into bytes.
   *
   * @returns UTF-8 JSON for every event in this transport batch.
   */
  toJsonBytes(): Uint8Array {
    return new TextEncoder().encode(this.#json);
  }
}

/**
 * Snapshot facade for one event inside a transferred batch.
 */
export class WorkerEvent {
  readonly #value: Record<string, unknown>;
  readonly #json: string;

  constructor(value: unknown) {
    if (value === null || typeof value !== "object") {
      throw new FinstackError("worker event snapshot is not an object", {
        code: "agent_run_invalid_configuration",
        retryable: false,
      });
    }
    this.#value = value as Record<string, unknown>;
    this.#json = JSON.stringify(value);
  }

  /** Event kind name such as `run_completed`. */
  get kind(): string {
    return typeof this.#value.kind === "string" ? this.#value.kind : "";
  }

  /** Durable or transient class inferred from the snapshot. */
  get eventClass(): string {
    return this.#value.durable_sequence === undefined || this.#value.durable_sequence === null
      ? "transient"
      : "durable_derived";
  }

  /** Transient sequence. */
  get transientSequence(): number {
    return typeof this.#value.transient_sequence === "number"
      ? this.#value.transient_sequence
      : 0;
  }

  /** Durable sequence, when the event is durable-derived. */
  get durableSequence(): number | undefined {
    return typeof this.#value.durable_sequence === "number"
      ? this.#value.durable_sequence
      : undefined;
  }

  /**
   * Return the transferred event JSON.
   *
   * @returns Canonical JSON text.
   */
  toJson(): string {
    return this.#json;
  }
}

/**
 * Main-thread proxy for one worker-hosted Agent.
 */
export class WorkerAgent {
  readonly #client: WorkerClient;
  readonly #agentId: string;

  constructor(client: WorkerClient, agentId: string) {
    this.#client = client;
    this.#agentId = agentId;
  }

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
  start(input: string, options?: RunOptions): WorkerRun {
    return this.#client.startRun(this.#agentId, input, options);
  }

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
  async run(input: string, options?: RunOptions): Promise<RunResultSnapshot> {
    return this.start(input, options).result();
  }
}

/**
 * Main-thread observation proxy for one worker-hosted run.
 *
 * Drop detaches observation and does not cancel execution.
 */
export class WorkerRun {
  readonly #client: WorkerClient;
  readonly #state: RunState;

  constructor(client: WorkerClient, state: RunState) {
    this.#client = client;
    this.#state = state;
  }

  /**
   * Session locator captured when the worker accepted the run.
   *
   * @throws {FinstackError} When start has not been acknowledged yet.
   */
  get session(): SessionSnapshot {
    if (this.#state.session === undefined) {
      throw new FinstackError("worker run has not started", {
        code: "agent_run_runtime_failure",
        retryable: false,
      });
    }
    return this.#state.session;
  }

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
  events(options?: EventOptions): AsyncIterable<WorkerEventBatch> {
    const state = this.#state;
    const client = this.#client;
    const signal = options?.signal;
    return {
      [Symbol.asyncIterator](): AsyncIterator<WorkerEventBatch> {
        let closed = signal?.aborted === true;
        state.iterating = !closed;
        const onAbort = (): void => {
          closed = true;
          state.iterating = false;
          void client.closeEvents(state);
        };
        signal?.addEventListener("abort", onAbort, { once: true });
        return {
          async next(): Promise<IteratorResult<WorkerEventBatch>> {
            await state.started.catch(() => undefined);
            while (!closed && !signal?.aborted) {
              const batch = state.inbox.shift();
              if (batch !== undefined) {
                client.ack(state, batch.lastSequence);
                return { done: false, value: batch };
              }
              if (state.error !== undefined || state.result !== undefined) {
                return { done: true, value: undefined };
              }
              await new Promise<void>((resolve) => {
                state.waiters.push(resolve);
              });
            }
            return { done: true, value: undefined };
          },
          async return(): Promise<IteratorResult<WorkerEventBatch>> {
            closed = true;
            state.iterating = false;
            signal?.removeEventListener("abort", onAbort);
            await client.closeEvents(state);
            return { done: true, value: undefined };
          },
        };
      },
    };
  }

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
  async result(): Promise<RunResultSnapshot> {
    return this.#state.finished;
  }

  /** Read the worker-hosted run's latest confirmed state. */
  async liveState(): Promise<RunStateSnapshot> {
    await this.#state.started;
    const message = await this.#client.request({
      v: 1,
      type: "liveState",
      id: this.#client.nextId(),
      agentId: this.#state.agentId,
      runId: this.#state.runId as string,
    });
    if (message.type !== "state" || message.snapshot === undefined) {
      throw new FinstackError("worker live state response is invalid", {
        code: "agent_run_runtime_failure",
        retryable: false,
      });
    }
    return message.snapshot;
  }

  /** Wait until the worker-hosted latest-only view advances. */
  async waitForLiveState(revision: number): Promise<RunStateSnapshot> {
    await this.#state.started;
    const message = await this.#client.request({
      v: 1,
      type: "waitForLiveState",
      id: this.#client.nextId(),
      agentId: this.#state.agentId,
      runId: this.#state.runId as string,
      revision,
    });
    if (message.type !== "state" || message.snapshot === undefined) {
      throw new FinstackError("worker live state response is invalid", {
        code: "agent_run_runtime_failure",
        retryable: false,
      });
    }
    return message.snapshot;
  }

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
  async cancel(): Promise<void> {
    await this.#state.started.catch(() => undefined);
    if (this.#state.runId === undefined) {
      return;
    }
    await this.#client.request({
      v: 1,
      type: "cancel",
      id: this.#client.nextId(),
      agentId: this.#state.agentId,
      runId: this.#state.runId,
    });
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
  async closeEvents(): Promise<void> {
    await this.#client.closeEvents(this.#state);
  }

  /**
   * Read the latest worker-to-UI queue pressure.
   *
   * @returns Queued batch count, dropped progress, and active policy.
   * @example
   * ```ts
   * const pressure = run.backpressure();
   * ```
   */
  backpressure(): WorkerBackpressure {
    return { ...this.#state.backpressure };
  }
}

/**
 * Main-thread client for one Dedicated Worker that hosts WASM Agents.
 *
 * This object does not call `init()` and does not load the wasm module.
 */
export class WorkerClient {
  readonly #worker: Worker;
  readonly #pending = new Map<string, Pending>();
  readonly #runs = new Map<string, RunState>();
  readonly #starting = new Map<string, RunState>();
  #next = 0;
  #closed = false;
  readonly #policy: LagPolicy;

  constructor(worker: Worker, policy: LagPolicy) {
    this.#worker = worker;
    this.#policy = policy;
    worker.addEventListener("message", (event) => {
      this.#onMessage(event);
    });
    worker.addEventListener("error", (event) => {
      const message = event instanceof ErrorEvent ? event.message : "";
      if (message.includes("closure invoked recursively or after being dropped")) {
        return;
      }
      this.#failAll(
        new FinstackError("worker crashed", {
          code: "agent_run_runtime_failure",
          retryable: false,
        }),
      );
    });
    worker.addEventListener("messageerror", () => {
      this.#failAll(
        new FinstackError("worker message failed", {
          code: "agent_run_runtime_failure",
          retryable: false,
        }),
      );
    });
  }

  /** Allocate a unique protocol request id. */
  nextId(): string {
    this.#next += 1;
    return `c-${this.#next}`;
  }

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
  async inspectSession(sessionId: string): Promise<SessionInspectSnapshot> {
    const inspected = await this.request({
      v: 1,
      type: "inspect",
      id: this.nextId(),
      sessionId,
    });
    if (inspected.type !== "inspected") {
      throw new FinstackError("worker inspect did not return a snapshot", {
        code: "agent_run_runtime_failure",
        retryable: false,
      });
    }
    return inspected.snapshot;
  }

  async create(options?: unknown): Promise<WorkerAgent> {
    const created = await this.request({
      v: 1,
      type: "create",
      id: this.nextId(),
      ...(options === undefined ? {} : { options }),
    });
    if (created.type !== "created") {
      throw new FinstackError("worker create did not return an agent", {
        code: "agent_run_runtime_failure",
        retryable: false,
      });
    }
    return new WorkerAgent(this, created.agentId);
  }

  /**
   * Cancel in-flight runs and terminate the Dedicated Worker.
   *
   * @returns A promise that settles after termination is requested.
   * @example
   * ```ts
   * await client.terminate();
   * ```
   */
  async terminate(): Promise<void> {
    if (this.#closed) {
      return;
    }
    this.#closed = true;
    const cancelled = new FinstackError("worker terminated", {
      code: "agent_run_cancelled",
      retryable: false,
    });
    for (const state of this.#runs.values()) {
      if (state.runId !== undefined) {
        const withContext = new FinstackError(cancelled.message, {
          code: cancelled.code,
          retryable: false,
          ...(state.session === undefined ? {} : { context: state.session }),
        });
        state.rejectFinished(withContext);
      } else {
        state.rejectFinished(cancelled);
      }
    }
    try {
      this.#post({ v: 1, type: "shutdown", id: this.nextId() });
    } catch {
      // The worker may already be gone.
    }
    this.#worker.terminate();
    this.#failAll(cancelled);
  }

  startRun(agentId: string, input: string, options?: RunOptions): WorkerRun {
    let resolveStarted!: (session: SessionSnapshot) => void;
    let rejectStarted!: (error: FinstackError) => void;
    let resolveFinished!: (snapshot: RunResultSnapshot) => void;
    let rejectFinished!: (error: FinstackError) => void;
    let startedSettled = false;
    let finishedSettled = false;
    const started = new Promise<SessionSnapshot>((resolve, reject) => {
      resolveStarted = (session) => {
        if (!startedSettled) {
          startedSettled = true;
          resolve(session);
        }
      };
      rejectStarted = (error) => {
        if (!startedSettled) {
          startedSettled = true;
          reject(error);
        }
      };
    });
    void started.catch(() => undefined);
    const finished = new Promise<RunResultSnapshot>((resolve, reject) => {
      resolveFinished = (snapshot) => {
        if (!finishedSettled) {
          finishedSettled = true;
          resolve(snapshot);
        }
      };
      rejectFinished = (error) => {
        if (!finishedSettled) {
          finishedSettled = true;
          reject(error);
        }
      };
    });
    const state: RunState = {
      agentId,
      inbox: [],
      waiters: [],
      closed: false,
      iterating: false,
      backpressure: {
        queuedBatches: 0,
        droppedProgress: 0,
        policy: this.#policy,
      },
      started,
      resolveStarted,
      rejectStarted,
      finished,
      resolveFinished,
      rejectFinished,
    };
    const id = this.nextId();
    this.#starting.set(id, state);
    this.#post({
      v: 1,
      type: "start",
      id,
      agentId,
      input,
      ...(options === undefined ? {} : { options }),
    });
    return new WorkerRun(this, state);
  }

  ack(state: RunState, lastSequence: number): void {
    if (state.runId === undefined || this.#closed) {
      return;
    }
    this.#post({
      v: 1,
      type: "ack",
      agentId: state.agentId,
      runId: state.runId,
      lastSequence,
    });
  }

  async closeEvents(state: RunState): Promise<void> {
    state.closed = true;
    state.iterating = false;
    if (state.runId === undefined || this.#closed) {
      return;
    }
    await this.request({
      v: 1,
      type: "closeEvents",
      id: this.nextId(),
      agentId: state.agentId,
      runId: state.runId,
    }).catch(() => undefined);
  }

  async request(message: MainToWorker): Promise<WorkerToMain> {
    if (!("id" in message) || message.id === undefined) {
      throw new FinstackError("worker request missing id", {
        code: "agent_run_invalid_configuration",
        retryable: false,
      });
    }
    const id = message.id;
    return new Promise((resolve, reject) => {
      this.#pending.set(id, { resolve, reject });
      try {
        this.#post(message);
      } catch (error) {
        this.#pending.delete(id);
        reject(FinstackError.fromUnknown(error));
      }
    });
  }

  #post(message: MainToWorker): void {
    this.#worker.postMessage(JSON.parse(encodeControl(message)));
  }

  #onMessage(event: MessageEvent): void {
    try {
      const payload = event.data;
      let envelope: unknown = payload;
      let bytes: ArrayBuffer | undefined;
      if (payload !== null && typeof payload === "object" && "envelope" in payload) {
        const record = payload as { envelope: unknown; bytes?: unknown };
        envelope = record.envelope;
        if (record.bytes !== undefined) {
          bytes = requireTransfer(record.bytes);
        }
      }
      const message = decodeWorkerToMain(envelope);
      this.#dispatch(message, bytes);
    } catch (error) {
      this.#failAll(FinstackError.fromUnknown(error));
    }
  }

  #dispatch(message: WorkerToMain, bytes?: ArrayBuffer): void {
    switch (message.type) {
      case "ready":
      case "created":
      case "inspected":
        this.#resolve(message.id, message);
        return;
      case "state": {
        if (bytes === undefined || bytes.byteLength > MAX_LIVE_STATE_BYTES) {
          throw new FinstackError("worker live state transfer is invalid", {
            code: "agent_run_invalid_configuration",
            retryable: false,
          });
        }
        const snapshot = JSON.parse(new TextDecoder().decode(bytes)) as RunStateSnapshot;
        this.#resolve(message.id, { ...message, snapshot });
        return;
      }
      case "started": {
        const state = this.#starting.get(message.id);
        this.#starting.delete(message.id);
        if (state !== undefined) {
          state.runId = message.runId;
          state.session = message.session;
          this.#runs.set(message.runId, state);
          state.resolveStarted(message.session);
        }
        this.#resolve(message.id, message);
        return;
      }
      case "batch": {
        const state = this.#runs.get(message.runId);
        if (state === undefined || bytes === undefined || state.closed) {
          return;
        }
        const batch = new WorkerEventBatch(
          bytes,
          message.firstSequence,
          message.lastSequence,
          message.droppedProgress,
        );
        // A durable-derived batch is never dropped, whether or not a consumer
        // has entered the iterator yet. `iterating` only flips true inside
        // `[Symbol.asyncIterator]()`, so the idiomatic
        // `await run.liveState(); for await (…)` leaves it false across the
        // liveState round trip — during which the worker's pump is already
        // posting. Acking there told the worker the batch was delivered, so it
        // was never resent and never counted as dropped progress.
        //
        // This mirrors the rule Rust's `EventLagPolicy::DropProgress` and the
        // worker's own `enqueue` already apply: drop transient, protect
        // durable. The worker stamps `durable` on the wire for exactly this.
        if (state.iterating || message.durable) {
          state.inbox.push(batch);
          this.#wake(state);
        } else {
          this.ack(state, message.lastSequence);
        }
        return;
      }
      case "result": {
        const state = this.#runs.get(message.runId);
        if (state !== undefined) {
          state.result = message.snapshot;
          state.resolveFinished(message.snapshot);
          this.#wake(state);
        }
        if (message.id !== undefined) {
          this.#resolve(message.id, message);
        }
        return;
      }
      case "error": {
        const error = new FinstackError(message.message, {
          code: message.code,
          retryable: message.retryable,
          ...(message.context === undefined ? {} : { context: message.context }),
        });
        if (message.id !== undefined) {
          const starting = this.#starting.get(message.id);
          if (starting !== undefined) {
            this.#starting.delete(message.id);
            starting.error = error;
            starting.rejectStarted(error);
            starting.rejectFinished(error);
            this.#wake(starting);
          }
          this.#pending.get(message.id)?.reject(error);
          this.#pending.delete(message.id);
        }
        if (message.runId !== undefined) {
          const state = this.#runs.get(message.runId);
          if (state !== undefined) {
            state.error = error;
            state.rejectFinished(error);
            this.#wake(state);
          }
        }
        return;
      }
      case "backpressure": {
        const state = this.#runs.get(message.runId);
        if (state !== undefined) {
          state.backpressure = {
            queuedBatches: message.queuedBatches,
            droppedProgress: message.droppedProgress,
            policy: message.policy,
          };
        }
        return;
      }
      case "terminated":
        this.#failAll(
          new FinstackError(message.reason, {
            code: "agent_run_cancelled",
            retryable: false,
          }),
        );
        return;
      default: {
        const _exhaustive: never = message;
        throw new FinstackError(`unsupported worker event: ${String(_exhaustive)}`, {
          code: "agent_run_invalid_configuration",
          retryable: false,
        });
      }
    }
  }

  #resolve(id: string, message: WorkerToMain): void {
    const pending = this.#pending.get(id);
    if (pending !== undefined) {
      this.#pending.delete(id);
      pending.resolve(message);
    }
  }

  #wake(state: RunState): void {
    const waiters = state.waiters.splice(0);
    for (const waiter of waiters) {
      waiter();
    }
  }

  #failAll(error: FinstackError): void {
    for (const pending of this.#pending.values()) {
      pending.reject(error);
    }
    this.#pending.clear();
    for (const state of this.#runs.values()) {
      const context = state.session ?? sessionSnapshot(error.context);
      const wrapped =
        context === undefined
          ? error
          : new FinstackError(error.message, {
              code: error.code,
              retryable: error.retryable,
              context,
            });
      state.error = wrapped;
      state.rejectFinished(wrapped);
      this.#wake(state);
    }
    for (const state of this.#starting.values()) {
      state.error = error;
      state.rejectStarted(error);
      state.rejectFinished(error);
      this.#wake(state);
    }
  }
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
export async function connectWorker(
  worker: Worker,
  options: WorkerConnectOptions = {},
): Promise<WorkerClient> {
  const client = new WorkerClient(worker, options.lagPolicy ?? "drop-progress");
  const init: MainToWorker = {
    v: 1,
    type: "init",
    id: client.nextId(),
  };
  if (options.lagPolicy !== undefined) {
    init.lagPolicy = options.lagPolicy;
  }
  if (options.queueCapacity !== undefined) {
    init.queueCapacity = options.queueCapacity;
  }
  if (options.blockTimeoutMs !== undefined) {
    init.blockTimeoutMs = options.blockTimeoutMs;
  }
  if (options.durableTimeoutMs !== undefined) {
    init.durableTimeoutMs = options.durableTimeoutMs;
  }
  const ready = await client.request(init);
  if (ready.type !== "ready") {
    throw new FinstackError("worker handshake failed", {
      code: "agent_run_runtime_failure",
      retryable: false,
    });
  }
  return client;
}
