import {
  Agent as WasmAgent,
  Event as WasmEvent,
  EventBatch as WasmEventBatch,
  Run as WasmRun,
  RunResult as WasmRunResult,
  Session as WasmSession,
} from "../generated/finstack_ai_wasm.js";

import {
  requireWasm,
  wasmModelHandle,
  wasmToolsetHandle,
} from "./adapters.js";
import type { JsModel, JsToolset } from "./adapters.js";
import { FinstackError } from "./errors.js";
import type {
  EventOptions,
  RunOptions,
  RunResultSnapshot,
  SessionSnapshot,
} from "./errors.js";

export { FinstackError } from "./errors.js";
export type {
  EventOptions,
  RunOptions,
  RunResultSnapshot,
  SessionSnapshot,
} from "./errors.js";

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
export class Agent {
  readonly #handle: WasmAgent;

  constructor(handle: WasmAgent) {
    this.#handle = handle;
  }

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
  static async create(options: AgentOptions): Promise<Agent> {
    requireWasm();
    try {
      const handle = await WasmAgent.create(
        wasmModelHandle(options.model),
        (options.toolsets ?? []).map((toolset) => wasmToolsetHandle(toolset).cloneHandle()),
        options.instruction,
      );
      return new Agent(handle);
    } catch (error) {
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
  start(input: string, options?: RunOptions): Run {
    requireWasm();
    try {
      return new Run(
        this.#handle.start(
          input,
          options?.timeoutSeconds,
          options?.maxCycles,
          options?.maxOutputRetries,
        ),
      );
    } catch (error) {
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
  async run(input: string, options?: RunOptions): Promise<RunResult> {
    requireWasm();
    try {
      const handle = await this.#handle.run(
        input,
        options?.timeoutSeconds,
        options?.maxCycles,
        options?.maxOutputRetries,
      );
      return new RunResult(handle);
    } catch (error) {
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
  readonly #handle: WasmRun;

  constructor(handle: WasmRun) {
    this.#handle = handle;
  }

  /**
   * Immutable session locator for this run.
   */
  get session(): Session {
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
  events(options?: EventOptions): AsyncIterable<EventBatch> {
    const handle = this.#handle;
    const signal = options?.signal;
    return {
      [Symbol.asyncIterator](): AsyncIterator<EventBatch> {
        let closed = signal?.aborted === true;
        const onAbort = (): void => {
          closed = true;
          handle.closeEvents();
        };
        signal?.addEventListener("abort", onAbort, { once: true });
        return {
          async next(): Promise<IteratorResult<EventBatch>> {
            if (closed || signal?.aborted) {
              return { done: true, value: undefined };
            }
            try {
              const batch = await handle.nextEventBatch();
              if (batch === undefined) {
                return { done: true, value: undefined };
              }
              return { done: false, value: new EventBatch(batch) };
            } catch (error) {
              throw FinstackError.fromUnknown(error);
            }
          },
          async return(): Promise<IteratorResult<EventBatch>> {
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
  async result(): Promise<RunResult> {
    try {
      return new RunResult(await this.#handle.result());
    } catch (error) {
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
  async cancel(reason?: string): Promise<void> {
    try {
      await this.#handle.cancel(reason);
    } catch (error) {
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
  async closeEvents(): Promise<void> {
    await this.#handle.closeEvents();
  }
}

/**
 * Immutable identifiers for one accepted operation.
 */
export class Session {
  readonly #handle: WasmSession;

  constructor(handle: WasmSession) {
    this.#handle = handle;
  }

  /** Tenant scope captured at acceptance. */
  get tenantScope(): string {
    return this.#handle.tenantScope;
  }

  /** Session identity. */
  get sessionId(): string {
    return this.#handle.sessionId;
  }

  /** Lane identity. */
  get laneId(): string {
    return this.#handle.laneId;
  }

  /** Run identity. */
  get runId(): string {
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
  toDict(): SessionSnapshot {
    return this.#handle.toDict() as SessionSnapshot;
  }
}

/**
 * Immutable successful terminal result snapshot.
 */
export class RunResult {
  readonly #handle: WasmRunResult;

  constructor(handle: WasmRunResult) {
    this.#handle = handle;
  }

  /** Concatenated final assistant text. */
  get text(): string {
    return this.#handle.text;
  }

  /** Durable retry attempts consumed by this run. */
  get retryAttempts(): number {
    return this.#handle.retryAttempts;
  }

  /** Session locator for the completed run. */
  get session(): Session {
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
  toDict(): RunResultSnapshot {
    return this.#handle.toDict() as RunResultSnapshot;
  }
}

/**
 * Immutable runtime event handle. Getters do not serialize the world.
 */
export class Event {
  readonly #handle: WasmEvent;

  constructor(handle: WasmEvent) {
    this.#handle = handle;
  }

  /** Event kind name such as `run_completed`. */
  get kind(): string {
    return this.#handle.kind;
  }

  /** Durable or transient class. */
  get eventClass(): string {
    return this.#handle.eventClass;
  }

  /** Transient sequence. */
  get transientSequence(): number {
    return asNumber(this.#handle.transientSequence);
  }

  /** Durable sequence, when the event is durable-derived. */
  get durableSequence(): number | undefined {
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
  toJson(): string {
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
  readonly #handle: WasmEventBatch;

  constructor(handle: WasmEventBatch) {
    this.#handle = handle;
  }

  /** First contained sequence. */
  get firstSequence(): number {
    return asNumber(this.#handle.firstSequence);
  }

  /** Last contained sequence. */
  get lastSequence(): number {
    return asNumber(this.#handle.lastSequence);
  }

  /** Lag-dropped transient events since the previous batch. */
  get droppedProgress(): number {
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
  events(): Event[] {
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
  toJson(): string {
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
  toJsonBytes(): Uint8Array {
    return this.#handle.toJsonBytes();
  }
}

function asNumber(value: number | bigint): number {
  return typeof value === "bigint" ? Number(value) : value;
}
