import {
  Agent as WasmAgent,
  Event as WasmEvent,
  EventBatch as WasmEventBatch,
  Lane as WasmLane,
  Locator as WasmLocator,
  MemoryExternalIdentityMap as WasmMemoryExternalIdentityMap,
  Run as WasmRun,
  RunResult as WasmRunResult,
  Session as WasmSession,
} from "../generated/finstack_ai_wasm.js";

import {
  requireWasm,
  wasmContextProviderHandle,
  wasmJournalStoreHandle,
  wasmMiddlewareHandle,
  wasmModelHandle,
  wasmObserverHandle,
  wasmToolsetHandle,
} from "./adapters.js";
import type {
  JsContextProvider,
  JsJournalStore,
  JsMiddleware,
  JsModel,
  JsObserver,
  JsToolset,
} from "./adapters.js";
import { FinstackError } from "./errors.js";
import type {
  AttachmentOption,
  EventOptions,
  RunOptions,
  RunResultSnapshot,
  SessionSnapshot,
} from "./errors.js";

export { FinstackError } from "./errors.js";
export type {
  AttachmentOption,
  EventOptions,
  RunOptions,
  RunResultSnapshot,
  SessionSnapshot,
} from "./errors.js";

/**
 * Declarative capability delivery mode. Rust owns activation semantics.
 */
export type CapabilityActivation = "always" | "application" | "model" | "disabled";

/**
 * Instruction-only alpha capability. Native bundles may also contribute
 * registered toolsets, context providers, and middleware.
 */
export interface Capability {
  /** Namespaced capability identity. */
  id: string;
  /** Compact non-secret description. */
  description: string;
  /** Instructions contributed after activation. */
  instructions: string[];
  /**
   * Delivery mode. Defaults to `application`.
   */
  activation?: CapabilityActivation;
}

/**
 * One compact model-visible capability catalog entry.
 */
export interface CapabilityCatalogItem {
  /** Capability identity. */
  id: string;
  /** Compact non-secret description. */
  description: string;
}

/**
 * One Rust-owned capability activation committed for a run.
 */
export interface ActiveCapability {
  /** Capability identity. */
  id: string;
  /** Selecting source. */
  source: "always" | "application" | "model";
}

/**
 * How a run parks and releases paid-tool approvals.
 *
 * Maps onto Rust `RunPolicy.approval_grant`. Defaults to `per_call`: one
 * park per unpaid Policy or Required tool call. `informed_batch` parks
 * once listing every unpaid paid tool. Neither mode relaxes the `Policy`
 * catalog floor.
 */
export type ApprovalGrantMode = "per_call" | "informed_batch";

/**
 * Options for {@link Agent.create}.
 *
 * Host objects inherit page authority and are not a sandbox.
 */
export interface AgentOptions {
  /** Trusted JS model wrapper. */
  model: JsModel;
  /** Optional trusted toolset wrappers. */
  toolsets?: JsToolset[];
  /** Optional model instruction. */
  instruction?: string;
  /**
   * Optional host journal. When omitted, the Rust in-memory store is used.
   * Persistence remains experimental after PR-048; it does not meet NFR-REL-001.
   */
  store?: JsJournalStore;
  /**
   * Optional declarative capabilities. JS capabilities contribute instructions.
   * Pass a model-activation id to {@link Agent.start} / {@link Agent.run}
   * via run options; unknown ids fail closed.
   */
  capabilities?: Capability[];
  /**
   * Application-activated capability IDs. Model-activation IDs fail closed.
   */
  activeCapabilities?: string[];
  /** Optional trusted context-provider wrappers. */
  contextProviders?: JsContextProvider[];
  /** Optional trusted middleware wrappers. */
  middleware?: JsMiddleware[];
  /** Optional trusted observer wrappers. */
  observers?: JsObserver[];
  /**
   * Optional paid-tool approval grant mode. Defaults to `per_call`.
   */
  approvalGrant?: ApprovalGrantMode;
}

/**
 * Provisional inspect phase for a stored session.
 *
 * `in_progress` and `cancelled` are valid after interrupt or reload. This is
 * not the PR-048 durable-beta crash-prefix matrix.
 */
export type SessionInspectPhase =
  | "empty"
  | "in_progress"
  | "completed"
  | "failed"
  | "cancelled";

/**
 * Replay-derived inspect snapshot. This is not a continue credential.
 */
export interface SessionInspectSnapshot {
  /** Stored session identity. */
  sessionId: string;
  /** Current committed head; zero denotes an empty journal. */
  headSequence: number;
  /** Provisional phase reconstructed from committed records. */
  phase: SessionInspectPhase;
  /** Concatenated final assistant text when the session completed. */
  resultText?: string;
  /** Kind name of the last committed record. */
  lastRecordKind?: string;
}

/**
 * Rust-owned resolved agent handle.
 */
export class Agent {
  readonly #handle: WasmAgent;

  constructor(handle: WasmAgent) {
    this.#handle = handle;
  }

  /** @internal */
  handle(): WasmAgent {
    return this.#handle;
  }

  /**
   * Construct an Agent over a trusted {@link JsModel} and optional toolsets.
   *
   * The default journal is the Rust in-memory store. Pass {@link AgentOptions.store}
   * to opt into a host journal. State remains in WASM until an explicit snapshot
   * or inspect. Reload restore is inspect, not continue-the-run.
   *
   * @param options - Model, optional toolsets, instruction, store, capabilities,
   * and approval grant.
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
   *   approvalGrant: "per_call",
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
        options.store === undefined
          ? undefined
          : wasmJournalStoreHandle(options.store).cloneHandle(),
        options.capabilities === undefined
          ? undefined
          : JSON.stringify(options.capabilities),
        options.activeCapabilities === undefined
          ? undefined
          : JSON.stringify(options.activeCapabilities),
        (options.contextProviders ?? []).map((provider) =>
          wasmContextProviderHandle(provider),
        ),
        (options.middleware ?? []).map((middleware) =>
          wasmMiddlewareHandle(middleware),
        ),
        (options.observers ?? []).map((observer) => wasmObserverHandle(observer)),
        options.approvalGrant,
      );
      return new Agent(handle);
    } catch (error) {
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
  capabilityCatalog(): CapabilityCatalogItem[] {
    requireWasm();
    return this.#handle.capabilityCatalog() as CapabilityCatalogItem[];
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
  compactCapabilityCatalog(): string {
    requireWasm();
    return this.#handle.compactCapabilityCatalog();
  }

  /**
   * Compose a new agent from reconstructed catalogs.
   *
   * In-flight runs keep the previous lock. wasm-host fail-closed is a Rust
   * platform error, not a missing method.
   */
  async reResolve(): Promise<Agent> {
    requireWasm();
    try {
      const handle = await this.#handle.reResolve();
      return new Agent(handle as WasmAgent);
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
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
  static async inspectSession(
    store: JsJournalStore,
    sessionId: string,
  ): Promise<SessionInspectSnapshot> {
    requireWasm();
    try {
      const snapshot = (await WasmAgent.inspectSession(
        wasmJournalStoreHandle(store),
        sessionId,
      )) as SessionInspectSnapshot;
      return {
        ...snapshot,
        headSequence: Number(snapshot.headSequence),
      };
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Create a live session on this agent's journal store.
   *
   * @param tenantScope - Tenant scope captured by the host.
   * @returns A live session handle with a bootstrapped `main` lane.
   * @throws {FinstackError} When the session cannot be created.
   * @example
   * ```ts
   * const session = await agent.createSession("tenant-a");
   * ```
   */
  async createSession(tenantScope = "default"): Promise<Session> {
    requireWasm();
    try {
      return new Session(await this.#handle.createSession(tenantScope));
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Open an existing session without respawning parked runs.
   *
   * @param sessionId - Durable session identity.
   * @param tenantScope - Tenant scope captured by the host.
   * @returns A live session handle rebuilt from the journal.
   * @throws {FinstackError} When the session cannot be opened.
   * @example
   * ```ts
   * const opened = await agent.openSession(session.sessionId, "tenant-a");
   * ```
   */
  async openSession(sessionId: string, tenantScope = "default"): Promise<Session> {
    requireWasm();
    try {
      return new Session(await this.#handle.openSession(sessionId, tenantScope));
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
   * @param options - Optional timeout, cycle, retry, and capability selection.
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
          options?.capability,
          options?.attachments,
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
   * @param options - Optional timeout, cycle, retry, and capability selection.
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
        options?.capability,
        options?.attachments,
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
   * Live session handle for this run.
   */
  get session(): Session {
    return new Session(this.#handle.session);
  }

  /**
   * Immutable operation locator snapshot.
   */
  get locator(): Locator {
    return new Locator(this.#handle.locator);
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
   * Prepare and accept one child through the Rust router.
   *
   * wasm-host fails closed with `agent_run_unsupported_plan` from Rust.
   *
   * @param agent - Child agent composition.
   * @param input - Child user text.
   * @param options - Placement and optional remote route. Remote placement
   * requires `routeEndpoint`, `routeService`, and `routeId`.
   * @returns The live child run handle on native hosts.
   * @throws {FinstackError} When prepare or accept fails.
   * @example
   * ```ts
   * const child = await parent.startChild(childAgent, "hello", {
   *   placement: "remote_child_session",
   *   routeEndpoint: "127.0.0.1:9",
   *   routeService: "finstack.remote.worker",
   *   routeId: "route-1",
   * });
   * ```
   */
  async startChild(
    agent: Agent,
    input: string,
    options?: {
      placement?: string;
      routeEndpoint?: string;
      routeService?: string;
      routeId?: string;
      routeToken?: string;
    },
  ): Promise<Run> {
    requireWasm();
    try {
      const handle = await this.#handle.startChild(
        agent.handle(),
        input,
        options?.placement ?? "isolated_child_session",
        options?.routeEndpoint,
        options?.routeService,
        options?.routeId,
        options?.routeToken,
      );
      return new Run(handle);
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
export class Locator {
  readonly #handle: WasmLocator;

  constructor(handle: WasmLocator) {
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
   */
  toDict(): SessionSnapshot {
    return this.#handle.toDict() as SessionSnapshot;
  }
}

/**
 * Inspect snapshot for one live lane.
 */
export interface LaneInspectSnapshot {
  /** Durable lane identity. */
  laneId: string;
  /** Stable application name. */
  name: string;
  /** History length ending at the current leaf. */
  historyLen: number;
}

/**
 * Host-owned `(channel, account, thread)` resolution.
 */
export interface ExternalIdentitySnapshot {
  /** Bound session identity. */
  sessionId: string;
  /** Bound lane identity. */
  laneId: string;
}

/**
 * Live handle for one journaled session.
 */
export class Session {
  readonly #handle: WasmSession;

  constructor(handle: WasmSession) {
    this.#handle = handle;
  }

  /** Tenant scope captured by the host. */
  get tenantScope(): string {
    return this.#handle.tenantScope;
  }

  /** Session identity. */
  get sessionId(): string {
    return this.#handle.sessionId;
  }

  /**
   * Create a named lane, optionally forking from an existing entry.
   *
   * @param name - Application lane name. `main` is reserved for bootstrap.
   * @param fork - Optional existing entry identity to share without copying.
   * @returns The new live lane handle.
   * @throws {FinstackError} When the name is invalid or the fork is unknown.
   * @example
   * ```ts
   * const research = await session.createLane("research", leafId);
   * ```
   */
  async createLane(name: string, fork?: string): Promise<Lane> {
    try {
      return new Lane(await this.#handle.createLane(name, fork));
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * List restored lanes.
   *
   * @returns Live handles for every lane in the session.
   * @throws {FinstackError} When the session cannot be loaded.
   * @example
   * ```ts
   * const lanes = await session.listLanes();
   * ```
   */
  async listLanes(): Promise<Lane[]> {
    try {
      const lanes = (await this.#handle.listLanes()) as WasmLane[];
      return lanes.map((lane) => new Lane(lane));
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Look up one lane by application name.
   *
   * @param name - Application lane name.
   * @returns The live lane handle.
   * @throws {FinstackError} When the lane does not exist.
   * @example
   * ```ts
   * const main = await session.lane("main");
   * ```
   */
  async lane(name: string): Promise<Lane> {
    try {
      return new Lane(await this.#handle.lane(name));
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Bind a host-owned external identity to one lane.
   *
   * @param map - In-process identity map owned by the host.
   * @param channel - Channel or adapter name.
   * @param account - Account identity on that channel.
   * @param thread - Conversation or thread identity.
   * @param laneId - Durable lane identity in this session.
   * @throws {FinstackError} When the key conflicts or the lane is unknown.
   * @example
   * ```ts
   * const map = new MemoryExternalIdentityMap();
   * session.bindExternalIdentity(map, "slack", "acct", "thread-1", main.laneId);
   * ```
   */
  bindExternalIdentity(
    map: MemoryExternalIdentityMap,
    channel: string,
    account: string,
    thread: string,
    laneId: string,
  ): void {
    try {
      const identityMap = identityMapHandles.get(map);
      if (identityMap === undefined) {
        throw new FinstackError("identity map is not initialized", {
          code: "invalid_identity_key",
          retryable: false,
        });
      }
      this.#handle.bindExternalIdentity(identityMap, channel, account, thread, laneId);
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }
}

/**
 * Live handle for one lane in a session.
 */
export class Lane {
  readonly #handle: WasmLane;

  constructor(handle: WasmLane) {
    this.#handle = handle;
  }

  /** Durable lane identity. */
  get laneId(): string {
    return this.#handle.laneId;
  }

  /** Session that owns this lane. */
  get session(): Session {
    return new Session(this.#handle.session);
  }

  /**
   * Point this idle lane at an existing entry without copying.
   *
   * @param entryId - Existing conversation entry identity.
   * @throws {FinstackError} When the entry is unknown or the lane is busy.
   * @example
   * ```ts
   * await research.navigate(leafId);
   * ```
   */
  async navigate(entryId: string): Promise<void> {
    try {
      await this.#handle.navigate(entryId);
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Inspect name, leaf, and history length.
   *
   * @returns A snapshot of the restored lane.
   * @throws {FinstackError} When the lane cannot be inspected.
   * @example
   * ```ts
   * const inspect = await research.inspect();
   * ```
   */
  async inspect(): Promise<LaneInspectSnapshot> {
    try {
      return (await this.#handle.inspect()) as LaneInspectSnapshot;
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Start a new root run on this idle lane.
   *
   * @param agent - Agent that owns the run plan and ports.
   * @param input - Plain-text user input.
   * @param options - Optional timeout, cycle, retry, and capability selection.
   * @returns A shared run handle.
   * @throws {FinstackError} When the lane is busy or the request is invalid.
   * @example
   * ```ts
   * const run = await research.run(agent, "hello");
   * ```
   */
  run(agent: Agent, input: string, options?: RunOptions): Run {
    try {
      return new Run(
        this.#handle.run(
          agent.handle(),
          input,
          options?.timeoutSeconds,
          options?.maxCycles,
          options?.maxOutputRetries,
          options?.capability,
          options?.attachments,
        ),
      );
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Park is unsupported on wasm-host.
   *
   * @throws {FinstackError} Always, with code `agent_run_unsupported_plan`.
   */
  async suspend(): Promise<void> {
    try {
      await this.#handle.suspend();
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }

  /**
   * Resume is unsupported on wasm-host.
   *
   * @param _agent - Accepted for API parity with native `Lane.resume`.
   * @throws {FinstackError} Always, with code `agent_run_unsupported_plan`.
   */
  async resume(_agent: Agent): Promise<void> {
    try {
      await this.#handle.resume(_agent.handle());
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
  }
}

const identityMapHandles = new WeakMap<
  MemoryExternalIdentityMap,
  WasmMemoryExternalIdentityMap
>();

/**
 * In-process external identity map.
 */
export class MemoryExternalIdentityMap {
  constructor(handle?: WasmMemoryExternalIdentityMap) {
    identityMapHandles.set(this, handle ?? new WasmMemoryExternalIdentityMap());
  }

  /**
   * Resolve one previously bound key.
   *
   * @param channel - Channel or adapter name.
   * @param account - Account identity on that channel.
   * @param thread - Conversation or thread identity.
   * @returns The bound session and lane, or `undefined`.
   * @throws {FinstackError} When the key is invalid.
   * @example
   * ```ts
   * const bound = map.resolve("slack", "acct", "thread-1");
   * ```
   */
  resolve(
    channel: string,
    account: string,
    thread: string,
  ): ExternalIdentitySnapshot | undefined {
    try {
      const bound = identityMapHandles.get(this)?.resolve(channel, account, thread) as
        | ExternalIdentitySnapshot
        | undefined;
      return bound;
    } catch (error) {
      throw FinstackError.fromUnknown(error);
    }
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

  /**
   * Stable Rust-owned committed record-kind trace in journal order.
   */
  get trace(): string[] {
    return Array.from(this.#handle.trace);
  }

  /**
   * Complete Rust-owned capability activation set for this run.
   */
  get activeCapabilities(): ActiveCapability[] {
    return this.#handle.activeCapabilities as ActiveCapability[];
  }

  /** Locator snapshot for the completed run. */
  get locator(): Locator {
    return new Locator(this.#handle.locator);
  }

  /** Locator snapshot for the completed run. */
  get session(): Locator {
    return new Locator(this.#handle.session);
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
