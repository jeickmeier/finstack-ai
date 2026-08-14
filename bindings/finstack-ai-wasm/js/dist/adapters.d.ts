import type { HostArtifactStore, HostClock, HostContextProvider, HostJournalStore, HostMiddleware, HostModel, HostObserver, HostRandomSource, HostToolset } from "./host.js";
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
/**
 * Record that {@link init} has completed so wrappers may construct wasm handles.
 *
 * @param value - Whether the generated module is ready.
 */
export declare function setAdaptersInitialized(value: boolean): void;
/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export declare class JsModel {
    #private;
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
    constructor(adapter: HostModel, options: JsModelOptions);
}
/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export declare class JsToolset {
    #private;
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
    constructor(adapter: HostToolset, options: JsToolsetOptions);
}
/**
 * Trusted JS context-provider wrapper.
 */
export declare class JsContextProvider {
    #private;
    /**
     * Wrap a trusted {@link HostContextProvider}.
     *
     * @param adapter - Host object whose `collect` returns a contribution.
     * @param options - Component identity and trust flag.
     * @throws When {@link init} has not completed or options are invalid.
     */
    constructor(adapter: HostContextProvider, options: JsContextProviderOptions);
}
/**
 * Trusted JS middleware wrapper.
 */
export declare class JsMiddleware {
    #private;
    /**
     * Wrap a trusted {@link HostMiddleware}.
     *
     * @param adapter - Host object whose `invoke` returns a stage outcome.
     * @param options - Component identity and stages.
     * @throws When {@link init} has not completed or stages are empty.
     */
    constructor(adapter: HostMiddleware, options: JsMiddlewareOptions);
}
/**
 * Trusted JS observer wrapper.
 */
export declare class JsObserver {
    #private;
    /**
     * Wrap a trusted {@link HostObserver}.
     *
     * @param adapter - Host object whose `observe` receives a projected batch.
     * @param options - Component identity and payload mode.
     * @throws When {@link init} has not completed or options are invalid.
     */
    constructor(adapter: HostObserver, options: JsObserverOptions);
}
/**
 * Pre-beta in-memory journal-store wrapper. Not crash-durable.
 */
export declare class JsJournalStore {
    #private;
    /**
     * Wrap a trusted {@link HostJournalStore}.
     *
     * IndexedDB and crash durability are later pull requests. This wrapper is
     * scripted / in-memory only.
     *
     * @param adapter - Host object that reports non-durable health.
     * @param options - Optional detail label.
     * @throws When {@link init} has not completed or the adapter is invalid.
     */
    constructor(adapter: HostJournalStore, options?: JsJournalStoreOptions);
}
/**
 * Host clock wrapper.
 */
export declare class JsClock {
    #private;
    /**
     * Wrap a trusted {@link HostClock}.
     *
     * @param adapter - Host object whose `now` returns Unix milliseconds.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter: HostClock);
}
/**
 * Host random-source wrapper.
 */
export declare class JsRandomSource {
    #private;
    /**
     * Wrap a trusted {@link HostRandomSource}.
     *
     * @param adapter - Host object whose `fillBytes` returns a `Uint8Array`.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter: HostRandomSource);
}
/**
 * Host artifact-store wrapper.
 */
export declare class JsArtifactStore {
    #private;
    /**
     * Wrap a trusted {@link HostArtifactStore}.
     *
     * @param adapter - Host object that stores and returns `Uint8Array` payloads.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter: HostArtifactStore);
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
export declare function createMemoryJournalStore(): HostJournalStore;
/**
 * Scripted in-memory artifact store over `Uint8Array` values.
 *
 * @returns A host store that keeps bytes in process memory.
 */
export declare function createMemoryArtifactStore(): HostArtifactStore;
/**
 * Host clock backed by `Date.now`, or an injected millisecond source.
 *
 * @param now - Optional Unix-ms source for tests.
 */
export declare function createHostClock(now?: () => number): HostClock;
/**
 * Host random source backed by `crypto.getRandomValues`.
 */
export declare function createHostRandomSource(): HostRandomSource;
//# sourceMappingURL=adapters.d.ts.map