import { JsArtifactStore as WasmJsArtifactStore, JsClock as WasmJsClock, JsContextProvider as WasmJsContextProvider, JsJournalStore as WasmJsJournalStore, JsMiddleware as WasmJsMiddleware, JsModel as WasmJsModel, JsObserver as WasmJsObserver, JsRandomSource as WasmJsRandomSource, JsToolset as WasmJsToolset, } from "../generated/finstack_ai_wasm.js";
function assertInitialized(flag) {
    if (!flag) {
        throw new Error("call init() before using @finstack/ai");
    }
}
let initialized = false;
const wasmModels = new WeakMap();
const wasmToolsets = new WeakMap();
const wasmContextProviders = new WeakMap();
const wasmMiddleware = new WeakMap();
const wasmObservers = new WeakMap();
const wasmStores = new WeakMap();
/**
 * Throw when the generated wasm module has not been initialized.
 *
 * @throws When {@link init} has not completed.
 */
export function requireWasm() {
    assertInitialized(initialized);
}
function getWasmHandle(map, wrapper, name) {
    const handle = map.get(wrapper);
    if (handle === undefined) {
        throw new TypeError(`${name} is not a live wasm handle`);
    }
    return handle;
}
/**
 * Return the crate-private wasm-bindgen model handle.
 *
 * @param model - Public {@link JsModel} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmModelHandle(model) {
    return getWasmHandle(wasmModels, model, "JsModel");
}
/**
 * Return the crate-private wasm-bindgen toolset handle.
 *
 * @param toolset - Public {@link JsToolset} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmToolsetHandle(toolset) {
    return getWasmHandle(wasmToolsets, toolset, "JsToolset");
}
/**
 * Return the crate-private wasm-bindgen context-provider handle.
 *
 * @param provider - Public {@link JsContextProvider} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmContextProviderHandle(provider) {
    return getWasmHandle(wasmContextProviders, provider, "JsContextProvider");
}
/**
 * Return the crate-private wasm-bindgen middleware handle.
 *
 * @param middleware - Public {@link JsMiddleware} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmMiddlewareHandle(middleware) {
    return getWasmHandle(wasmMiddleware, middleware, "JsMiddleware");
}
/**
 * Return the crate-private wasm-bindgen observer handle.
 *
 * @param observer - Public {@link JsObserver} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmObserverHandle(observer) {
    return getWasmHandle(wasmObservers, observer, "JsObserver");
}
/**
 * Return the crate-private wasm-bindgen journal-store handle.
 *
 * @param store - Public {@link JsJournalStore} wrapper.
 * @returns The generated wasm handle.
 * @throws When the wrapper was not constructed after {@link init}.
 */
export function wasmJournalStoreHandle(store) {
    return getWasmHandle(wasmStores, store, "JsJournalStore");
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
        wasmContextProviders.set(this, this.#handle);
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
        wasmMiddleware.set(this, this.#handle);
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
        wasmObservers.set(this, this.#handle);
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
 * Scripted bounded in-memory artifact store over `Uint8Array` values.
 *
 * @param options - Required finite store identity and capacities.
 * @returns A host store that keeps bytes in process memory.
 */
export function createMemoryArtifactStore(options) {
    if (options.storeId.length === 0 ||
        !Number.isSafeInteger(options.maxArtifactBytes) ||
        options.maxArtifactBytes < 0 ||
        !Number.isSafeInteger(options.maxArtifacts) ||
        options.maxArtifacts < 0 ||
        !Number.isSafeInteger(options.maxTotalBytes) ||
        options.maxTotalBytes < 0 ||
        !Number.isSafeInteger(options.maxOwnersPerArtifact) ||
        options.maxOwnersPerArtifact < 0 ||
        !Number.isSafeInteger(options.orphanGraceMs) ||
        options.orphanGraceMs < 0 ||
        !Number.isSafeInteger(options.maxGcBatch) ||
        options.maxGcBatch < 0) {
        throw new TypeError("invalid artifact store options");
    }
    const entries = new Map();
    let totalBytes = 0;
    return {
        async stagePut(scope, content, _metadata, artifact, storageKey) {
            const key = String(storageKey ?? "");
            const scopeJson = String(scope ?? "");
            const artifactJson = String(artifact ?? "");
            if (key.length === 0 || scopeJson.length === 0 || artifactJson.length === 0) {
                throw new TypeError("artifact identity missing");
            }
            if (content.byteLength > options.maxArtifactBytes) {
                throw new TypeError("artifact exceeds per-item capacity");
            }
            const existing = entries.get(key);
            if (existing) {
                const bytesMatch = existing.bytes.byteLength === content.byteLength &&
                    existing.bytes.every((value, index) => value === content[index]);
                if (existing.scope !== scopeJson ||
                    existing.artifact !== artifactJson ||
                    !bytesMatch) {
                    throw new TypeError("artifact identity collision");
                }
                return;
            }
            if (entries.size >= options.maxArtifacts) {
                throw new TypeError("artifact count capacity exceeded");
            }
            if (totalBytes + content.byteLength > options.maxTotalBytes) {
                throw new TypeError("artifact byte capacity exceeded");
            }
            const bytes = content.slice();
            entries.set(key, {
                scope: scopeJson,
                artifact: artifactJson,
                bytes,
                owners: new Set(),
            });
            totalBytes += bytes.byteLength;
        },
        async get(scope, artifact, storageKey) {
            const found = entries.get(String(storageKey));
            if (!found) {
                throw new TypeError("artifact missing");
            }
            if (found.scope !== String(scope) || found.artifact !== String(artifact)) {
                throw new TypeError("artifact scope or reference mismatch");
            }
            return found.bytes.slice();
        },
        async getByBlob(scope, blob) {
            const scopeJson = String(scope ?? "");
            const blobJson = String(blob ?? "");
            for (const entry of entries.values()) {
                if (entry.scope !== scopeJson)
                    continue;
                let artifact;
                try {
                    artifact = JSON.parse(entry.artifact);
                }
                catch {
                    throw new TypeError("stored artifact reference is invalid");
                }
                if (JSON.stringify(artifact.blob) === blobJson) {
                    return [entry.artifact, entry.bytes.slice()];
                }
            }
            throw new TypeError("artifact missing");
        },
        async pin(scope, artifact, storageKey, owner) {
            const found = entries.get(String(storageKey));
            if (!found) {
                throw new TypeError("artifact missing");
            }
            if (found.scope !== String(scope) || found.artifact !== String(artifact)) {
                throw new TypeError("artifact scope or reference mismatch");
            }
            const ownerId = String(owner ?? "");
            if (ownerId.length === 0) {
                throw new TypeError("artifact owner missing");
            }
            if (!found.owners.has(ownerId) && found.owners.size >= options.maxOwnersPerArtifact) {
                throw new TypeError("artifact owner capacity exceeded");
            }
            found.owners.add(ownerId);
            delete found.unreferencedSince;
        },
        async unpin(scope, artifact, storageKey, owner, nowUnixMs) {
            const found = entries.get(String(storageKey));
            if (!found) {
                throw new TypeError("artifact missing");
            }
            if (found.scope !== String(scope) || found.artifact !== String(artifact)) {
                throw new TypeError("artifact scope or reference mismatch");
            }
            if (found.owners.delete(String(owner)) && found.owners.size === 0) {
                found.unreferencedSince = Number(nowUnixMs);
            }
        },
        async collectOrphans(scope, nowUnixMs, limit) {
            const scopeJson = String(scope);
            const now = Number(nowUnixMs);
            const boundedLimit = Math.min(Number(limit), options.maxGcBatch);
            let examined = 0;
            let deleted = 0;
            let bytesDeleted = 0;
            for (const [key, entry] of entries) {
                if (examined >= boundedLimit)
                    break;
                if (entry.scope !== scopeJson)
                    continue;
                examined += 1;
                if (entry.owners.size > 0)
                    continue;
                if (entry.unreferencedSince === undefined) {
                    entry.unreferencedSince = now;
                    continue;
                }
                if (now - entry.unreferencedSince >= options.orphanGraceMs) {
                    entries.delete(key);
                    totalBytes -= entry.bytes.byteLength;
                    deleted += 1;
                    bytesDeleted += entry.bytes.byteLength;
                }
            }
            return { examined, deleted, bytes_deleted: bytesDeleted };
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