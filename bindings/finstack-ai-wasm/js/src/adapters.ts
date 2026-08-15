import {
  JsArtifactStore as WasmJsArtifactStore,
  JsClock as WasmJsClock,
  JsContextProvider as WasmJsContextProvider,
  JsJournalStore as WasmJsJournalStore,
  JsMiddleware as WasmJsMiddleware,
  JsModel as WasmJsModel,
  JsObserver as WasmJsObserver,
  JsRandomSource as WasmJsRandomSource,
  JsToolset as WasmJsToolset,
} from "../generated/finstack_ai_wasm.js";

import type {
  HostArtifactStore,
  HostClock,
  HostContextProvider,
  HostJournalStore,
  HostMiddleware,
  HostModel,
  HostObserver,
  HostRandomSource,
  HostToolset,
} from "./host.js";

/**
 * Constructor options for {@link JsModel}.
 */
export interface JsModelOptions {
  /** Exact component identity. */
  component: string;
  /** Provider label. */
  provider: string;
  /** Model name. */
  model: string;
  /** Canonical request byte ceiling. */
  hardInputBytes?: number;
  /** Context-window token ceiling. */
  contextWindowTokens?: number;
  /** Maximum output tokens. */
  maxOutputTokens?: number;
}

/**
 * Constructor options for {@link JsToolset}.
 */
export interface JsToolsetOptions {
  /** Exact component identity. */
  component: string;
  /** Toolset name. */
  name: string;
  /** Cached tool schemas. */
  tools: unknown[];
}

/**
 * Constructor options for {@link JsContextProvider}.
 */
export interface JsContextProviderOptions {
  /** Exact component identity. */
  component: string;
  /** Whether the provider may supply trusted application instructions. */
  trustedApplicationInstructions?: boolean;
}

/**
 * Constructor options for {@link JsMiddleware}.
 */
export interface JsMiddlewareOptions {
  /** Exact component identity. */
  component: string;
  /** Stage names such as `before_run` and `before_finalize`. */
  stages: string[];
  /** Standard-tier priority. */
  priority?: number;
}

/**
 * Constructor options for {@link JsObserver}.
 */
export interface JsObserverOptions {
  /** Exact component identity. */
  component: string;
  /** Payload projection mode. */
  payloadMode?: "metadata_only" | "redacted" | "full";
}

/**
 * Constructor options for {@link JsJournalStore}.
 */
export interface JsJournalStoreOptions {
  /** Stable non-secret operating-mode description. */
  detail?: string;
}

function assertInitialized(flag: boolean): void {
  if (!flag) {
    throw new Error("call init() before using @finstack/ai");
  }
}

let initialized = false;
const wasmModels = new WeakMap<JsModel, WasmJsModel>();
const wasmToolsets = new WeakMap<JsToolset, WasmJsToolset>();
const wasmContextProviders = new WeakMap<JsContextProvider, WasmJsContextProvider>();
const wasmMiddleware = new WeakMap<JsMiddleware, WasmJsMiddleware>();
const wasmObservers = new WeakMap<JsObserver, WasmJsObserver>();
const wasmStores = new WeakMap<JsJournalStore, WasmJsJournalStore>();

/**
 * Throw when the generated wasm module has not been initialized.
 *
 * @throws When {@link init} has not completed.
 */
export function requireWasm(): void {
  assertInitialized(initialized);
}

/**
 * Return the crate-private wasm-bindgen model handle.
 *
 * @param model - Public {@link JsModel} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmModelHandle(model: JsModel): WasmJsModel {
  const handle = wasmModels.get(model);
  if (handle === undefined) {
    throw new TypeError("JsModel is not a live wasm handle");
  }
  return handle;
}

/**
 * Return the crate-private wasm-bindgen toolset handle.
 *
 * @param toolset - Public {@link JsToolset} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmToolsetHandle(toolset: JsToolset): WasmJsToolset {
  const handle = wasmToolsets.get(toolset);
  if (handle === undefined) {
    throw new TypeError("JsToolset is not a live wasm handle");
  }
  return handle;
}

/**
 * Return the crate-private wasm-bindgen context-provider handle.
 *
 * @param provider - Public {@link JsContextProvider} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmContextProviderHandle(
  provider: JsContextProvider,
): WasmJsContextProvider {
  const handle = wasmContextProviders.get(provider);
  if (handle === undefined) {
    throw new TypeError("JsContextProvider is not a live wasm handle");
  }
  return handle;
}

/**
 * Return the crate-private wasm-bindgen middleware handle.
 *
 * @param middleware - Public {@link JsMiddleware} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmMiddlewareHandle(middleware: JsMiddleware): WasmJsMiddleware {
  const handle = wasmMiddleware.get(middleware);
  if (handle === undefined) {
    throw new TypeError("JsMiddleware is not a live wasm handle");
  }
  return handle;
}

/**
 * Return the crate-private wasm-bindgen observer handle.
 *
 * @param observer - Public {@link JsObserver} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmObserverHandle(observer: JsObserver): WasmJsObserver {
  const handle = wasmObservers.get(observer);
  if (handle === undefined) {
    throw new TypeError("JsObserver is not a live wasm handle");
  }
  return handle;
}

/**
 * Return the crate-private wasm-bindgen journal-store handle.
 *
 * @param store - Public {@link JsJournalStore} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmJournalStoreHandle(store: JsJournalStore): WasmJsJournalStore {
  const handle = wasmStores.get(store);
  if (handle === undefined) {
    throw new TypeError("JsJournalStore is not a live wasm handle");
  }
  return handle;
}

/**
 * Record that {@link init} has completed so wrappers may construct wasm handles.
 *
 * @param value - Whether the generated module is ready.
 */
export function setAdaptersInitialized(value: boolean): void {
  initialized = value;
}

/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export class JsModel {
  readonly #handle: WasmJsModel;

  /**
   * Wrap a trusted {@link HostModel}.
   *
   * @param adapter - Host object whose `request` returns a promise, stream, or iterable.
   * @param options - Component, provider, and model identity.
   * @throws When {@link init} has not completed or options are invalid.
   * @example
   * ```ts
   * const model = new JsModel(
   *   { request: async () => ({ text: "ok", completion_id: "1" }) },
   *   { component: "app.model", provider: "scripted", model: "scripted-model" },
   * );
   * ```
   */
  constructor(adapter: HostModel, options: JsModelOptions) {
    assertInitialized(initialized);
    this.#handle = new WasmJsModel(adapter, options);
    wasmModels.set(this, this.#handle);
  }
}

/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export class JsToolset {
  readonly #handle: WasmJsToolset;

  /**
   * Wrap a trusted {@link HostToolset}.
   *
   * @param adapter - Host object whose `call` returns a coarse tool result.
   * @param options - Component, name, and cached tool schemas.
   * @throws When {@link init} has not completed or tools are empty.
   * @example
   * ```ts
   * const toolset = new JsToolset(host, {
   *   component: "app.tools",
   *   name: "scripted-tools",
   *   tools: [echoTool],
   * });
   * ```
   */
  constructor(adapter: HostToolset, options: JsToolsetOptions) {
    assertInitialized(initialized);
    this.#handle = new WasmJsToolset(adapter, options);
    wasmToolsets.set(this, this.#handle);
  }
}

/**
 * Trusted JS context-provider wrapper.
 */
export class JsContextProvider {
  readonly #handle: WasmJsContextProvider;

  /**
   * Wrap a trusted {@link HostContextProvider}.
   *
   * @param adapter - Host object whose `collect` returns a contribution.
   * @param options - Component identity and trust flag.
   * @throws When {@link init} has not completed or options are invalid.
   */
  constructor(adapter: HostContextProvider, options: JsContextProviderOptions) {
    assertInitialized(initialized);
    this.#handle = new WasmJsContextProvider(adapter, options);
    wasmContextProviders.set(this, this.#handle);
  }
}

/**
 * Trusted JS middleware wrapper.
 */
export class JsMiddleware {
  readonly #handle: WasmJsMiddleware;

  /**
   * Wrap a trusted {@link HostMiddleware}.
   *
   * @param adapter - Host object whose `invoke` returns a stage outcome.
   * @param options - Component identity and stages.
   * @throws When {@link init} has not completed or stages are empty.
   */
  constructor(adapter: HostMiddleware, options: JsMiddlewareOptions) {
    assertInitialized(initialized);
    this.#handle = new WasmJsMiddleware(adapter, options);
    wasmMiddleware.set(this, this.#handle);
  }
}

/**
 * Trusted JS observer wrapper.
 */
export class JsObserver {
  readonly #handle: WasmJsObserver;

  /**
   * Wrap a trusted {@link HostObserver}.
   *
   * @param adapter - Host object whose `observe` receives a projected batch.
   * @param options - Component identity and payload mode.
   * @throws When {@link init} has not completed or options are invalid.
   */
  constructor(adapter: HostObserver, options: JsObserverOptions) {
    assertInitialized(initialized);
    this.#handle = new WasmJsObserver(adapter, options);
    wasmObservers.set(this, this.#handle);
  }
}

/**
 * Trusted JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
  readonly #handle: WasmJsJournalStore;

  /**
   * Wrap a trusted {@link HostJournalStore}.
   *
   * Persistence is opt-in through a host that implements `append` / `load` /
   * `writeSnapshot`. The default memory helper remains health-only.
   *
   * @param adapter - Host object that reports non-durable health.
   * @param options - Optional detail label.
   * @throws When {@link init} has not completed or the adapter is invalid.
   */
  constructor(adapter: HostJournalStore, options: JsJournalStoreOptions = {}) {
    assertInitialized(initialized);
    this.#handle = new WasmJsJournalStore(adapter, options);
    wasmStores.set(this, this.#handle);
  }
}

/**
 * Host clock wrapper.
 */
export class JsClock {
  readonly #handle: WasmJsClock;

  /**
   * Wrap a trusted {@link HostClock}.
   *
   * @param adapter - Host object whose `now` returns Unix milliseconds.
   * @throws When {@link init} has not completed.
   */
  constructor(adapter: HostClock) {
    assertInitialized(initialized);
    this.#handle = new WasmJsClock(adapter);
  }
}

/**
 * Host random-source wrapper.
 */
export class JsRandomSource {
  readonly #handle: WasmJsRandomSource;

  /**
   * Wrap a trusted {@link HostRandomSource}.
   *
   * @param adapter - Host object whose `fillBytes` returns a `Uint8Array`.
   * @throws When {@link init} has not completed.
   */
  constructor(adapter: HostRandomSource) {
    assertInitialized(initialized);
    this.#handle = new WasmJsRandomSource(adapter);
  }
}

/**
 * Host artifact-store wrapper.
 */
export class JsArtifactStore {
  readonly #handle: WasmJsArtifactStore;

  /**
   * Wrap a trusted {@link HostArtifactStore}.
   *
   * @param adapter - Host object that stores and returns `Uint8Array` payloads.
   * @throws When {@link init} has not completed.
   */
  constructor(adapter: HostArtifactStore) {
    assertInitialized(initialized);
    this.#handle = new WasmJsArtifactStore(adapter);
  }
}

/**
 * Scripted in-memory journal store. Pre-beta; not crash-durable.
 *
 * @returns A host store that reports `ready` and never claims durability.
 * @example
 * ```ts
 * const store = new JsJournalStore(createMemoryJournalStore());
 * ```
 */
export function createMemoryJournalStore(): HostJournalStore {
  return {
    async health() {
      return { ready: true, detail: "js_memory_prebeta" };
    },
  };
}

/**
 * Scripted in-memory artifact store over `Uint8Array` values.
 *
 * @returns A host store that keeps bytes in process memory.
 */
export function createMemoryArtifactStore(): HostArtifactStore {
  const entries = new Map<string, Uint8Array>();
  return {
    async stagePut(_scope, content, _metadata, id) {
      entries.set(String(id ?? content.length), content);
    },
    async get(key) {
      const found = entries.get(String(key));
      if (!found) {
        throw new TypeError("artifact missing");
      }
      return found;
    },
  };
}

/**
 * Host clock backed by `Date.now`, or an injected millisecond source.
 *
 * @param now - Optional Unix-ms source for tests.
 */
export function createHostClock(now: () => number = Date.now): HostClock {
  return { now };
}

/**
 * Host random source backed by `crypto.getRandomValues`.
 */
export function createHostRandomSource(): HostRandomSource {
  return {
    fillBytes(length: number): Uint8Array {
      const bytes = new Uint8Array(length);
      crypto.getRandomValues(bytes);
      return bytes;
    },
  };
}
