import { JsArtifactStore as WasmJsArtifactStore, JsClock as WasmJsClock, JsContextProvider as WasmJsContextProvider, JsJournalStore as WasmJsJournalStore, JsMiddleware as WasmJsMiddleware, JsModel as WasmJsModel, JsObserver as WasmJsObserver, JsRandomSource as WasmJsRandomSource, JsToolset as WasmJsToolset, } from "../generated/finstack_ai_wasm.js";
function assertInitialized(flag) {
    if (!flag) {
        throw new Error("call init() before using @finstack/ai");
    }
}
let initialized = false;
const wasmModels = new WeakMap();
const wasmToolsets = new WeakMap();
const wasmStores = new WeakMap();
/**
 * Throw when the generated wasm module has not been initialized.
 *
 * @throws When {@link init} has not completed.
 */
export function requireWasm() {
    assertInitialized(initialized);
}
/**
 * Return the crate-private wasm-bindgen model handle.
 *
 * @param model - Public {@link JsModel} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmModelHandle(model) {
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
export function wasmToolsetHandle(toolset) {
    const handle = wasmToolsets.get(toolset);
    if (handle === undefined) {
        throw new TypeError("JsToolset is not a live wasm handle");
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
export function wasmJournalStoreHandle(store) {
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
export function setAdaptersInitialized(value) {
    initialized = value;
}
/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export class JsModel {
    #handle;
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
    constructor(adapter, options) {
        assertInitialized(initialized);
        this.#handle = new WasmJsModel(adapter, options);
        wasmModels.set(this, this.#handle);
    }
}
/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export class JsToolset {
    #handle;
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
    constructor(adapter, options) {
        assertInitialized(initialized);
        this.#handle = new WasmJsToolset(adapter, options);
        wasmToolsets.set(this, this.#handle);
    }
}
/**
 * Trusted JS context-provider wrapper.
 */
export class JsContextProvider {
    #handle;
    /**
     * Wrap a trusted {@link HostContextProvider}.
     *
     * @param adapter - Host object whose `collect` returns a contribution.
     * @param options - Component identity and trust flag.
     * @throws When {@link init} has not completed or options are invalid.
     */
    constructor(adapter, options) {
        assertInitialized(initialized);
        this.#handle = new WasmJsContextProvider(adapter, options);
    }
}
/**
 * Trusted JS middleware wrapper.
 */
export class JsMiddleware {
    #handle;
    /**
     * Wrap a trusted {@link HostMiddleware}.
     *
     * @param adapter - Host object whose `invoke` returns a stage outcome.
     * @param options - Component identity and stages.
     * @throws When {@link init} has not completed or stages are empty.
     */
    constructor(adapter, options) {
        assertInitialized(initialized);
        this.#handle = new WasmJsMiddleware(adapter, options);
    }
}
/**
 * Trusted JS observer wrapper.
 */
export class JsObserver {
    #handle;
    /**
     * Wrap a trusted {@link HostObserver}.
     *
     * @param adapter - Host object whose `observe` receives a projected batch.
     * @param options - Component identity and payload mode.
     * @throws When {@link init} has not completed or options are invalid.
     */
    constructor(adapter, options) {
        assertInitialized(initialized);
        this.#handle = new WasmJsObserver(adapter, options);
    }
}
/**
 * Trusted JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
    #handle;
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
    constructor(adapter, options = {}) {
        assertInitialized(initialized);
        this.#handle = new WasmJsJournalStore(adapter, options);
        wasmStores.set(this, this.#handle);
    }
}
/**
 * Host clock wrapper.
 */
export class JsClock {
    #handle;
    /**
     * Wrap a trusted {@link HostClock}.
     *
     * @param adapter - Host object whose `now` returns Unix milliseconds.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter) {
        assertInitialized(initialized);
        this.#handle = new WasmJsClock(adapter);
    }
}
/**
 * Host random-source wrapper.
 */
export class JsRandomSource {
    #handle;
    /**
     * Wrap a trusted {@link HostRandomSource}.
     *
     * @param adapter - Host object whose `fillBytes` returns a `Uint8Array`.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter) {
        assertInitialized(initialized);
        this.#handle = new WasmJsRandomSource(adapter);
    }
}
/**
 * Host artifact-store wrapper.
 */
export class JsArtifactStore {
    #handle;
    /**
     * Wrap a trusted {@link HostArtifactStore}.
     *
     * @param adapter - Host object that stores and returns `Uint8Array` payloads.
     * @throws When {@link init} has not completed.
     */
    constructor(adapter) {
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
export function createMemoryJournalStore() {
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
export function createMemoryArtifactStore() {
    const entries = new Map();
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
export function createHostClock(now = Date.now) {
    return { now };
}
/**
 * Host random source backed by `crypto.getRandomValues`.
 */
export function createHostRandomSource() {
    return {
        fillBytes(length) {
            const bytes = new Uint8Array(length);
            crypto.getRandomValues(bytes);
            return bytes;
        },
    };
}
//# sourceMappingURL=adapters.js.map