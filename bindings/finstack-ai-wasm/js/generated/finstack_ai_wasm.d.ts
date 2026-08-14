/* tslint:disable */
/* eslint-disable */

/**
 * Host artifact-store wrapper over `Uint8Array` payloads.
 */
export class JsArtifactStore {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct an artifact-store wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `stagePut` or `get` is missing.
     */
    constructor(adapter: any);
}

/**
 * Host clock wrapper. `now()` returns Unix milliseconds.
 */
export class JsClock {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a clock wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `now` is missing.
     */
    constructor(adapter: any);
}

/**
 * Trusted JS context-provider wrapper.
 */
export class JsContextProvider {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a context-provider wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Pre-beta in-memory JS journal-store wrapper. Not crash-durable.
 */
export class JsJournalStore {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a scripted journal-store wrapper.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when the adapter is invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS middleware wrapper.
 */
export class JsMiddleware {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a middleware wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS model wrapper. Not an Agent handle.
 */
export class JsModel {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a model wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Trusted JS observer wrapper.
 */
export class JsObserver {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct an observer wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Host random-source wrapper.
 */
export class JsRandomSource {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a random-source wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when `fillBytes` is missing.
     */
    constructor(adapter: any);
}

/**
 * Trusted JS toolset wrapper. Not an Agent handle.
 */
export class JsToolset {
    free(): void;
    [Symbol.dispose](): void;
    /**
     * Construct a toolset wrapper around a trusted host adapter.
     *
     * # Errors
     *
     * Returns a TypeError-equivalent when options or the adapter are invalid.
     */
    constructor(adapter: any, options: any);
}

/**
 * Apply normalized coordinator commands and return identity traces.
 *
 * This export is test-only and does not submit a live Agent.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when any command fails DTO validation.
 */
export function applyScriptedCoordinatorCommands(encoded: string): string;

/**
 * Lockstep version metadata for the wasm package.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the metadata object cannot be constructed.
 */
export function buildMetadata(): any;

/**
 * Construct the six-port compile fixtures for the current target.
 */
export function compilePortProxies(): void;

/**
 * Drive scripted journal-store health. Test-only; always non-durable.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the adapter is invalid.
 */
export function driveScriptedJournalHealth(adapter: any, options: any): Promise<any>;

/**
 * Drive one scripted `Model::request`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 */
export function driveScriptedModelRequest(adapter: any, options: any, signal: any): Promise<any>;

/**
 * Drive one scripted `Toolset::call`. Test-only; not an Agent run.
 *
 * # Errors
 *
 * Returns a JavaScript exception when options cannot be parsed.
 */
export function driveScriptedToolCall(adapter: any, options: any, signal: any): Promise<any>;

/**
 * Process-local health token. Does not create a runtime, open a store, or spawn work.
 */
export function health(): string;

/**
 * Normalize a pre-beta lineage or authenticated external-command shape.
 *
 * # Errors
 *
 * Returns a TypeError-equivalent when `kind` is unsupported or `value` is invalid.
 */
export function normalizePrebetaShape(kind: string, encoded: string): string;

/**
 * Run the embedded no-op golden trace through `CommitCoordinator`.
 *
 * This export is a test-only engine-load proof and is not part of the public
 * TypeScript surface.
 *
 * # Errors
 *
 * Returns a JavaScript exception when the fixture, store, or report fails.
 */
export function runNoopTrace(): string;

export type InitInput = RequestInfo | URL | Response | BufferSource | WebAssembly.Module;

export interface InitOutput {
    readonly memory: WebAssembly.Memory;
    readonly __wbg_jsartifactstore_free: (a: number, b: number) => void;
    readonly __wbg_jsclock_free: (a: number, b: number) => void;
    readonly __wbg_jscontextprovider_free: (a: number, b: number) => void;
    readonly __wbg_jsjournalstore_free: (a: number, b: number) => void;
    readonly __wbg_jsmiddleware_free: (a: number, b: number) => void;
    readonly __wbg_jsmodel_free: (a: number, b: number) => void;
    readonly __wbg_jsobserver_free: (a: number, b: number) => void;
    readonly __wbg_jstoolset_free: (a: number, b: number) => void;
    readonly applyScriptedCoordinatorCommands: (a: number, b: number, c: number) => void;
    readonly buildMetadata: (a: number) => void;
    readonly compilePortProxies: () => void;
    readonly health: (a: number) => void;
    readonly jsartifactstore_new: (a: number, b: number) => void;
    readonly jsclock_new: (a: number, b: number) => void;
    readonly jscontextprovider_new: (a: number, b: number, c: number) => void;
    readonly jsjournalstore_new: (a: number, b: number, c: number) => void;
    readonly jsmiddleware_new: (a: number, b: number, c: number) => void;
    readonly jsmodel_new: (a: number, b: number, c: number) => void;
    readonly jsobserver_new: (a: number, b: number, c: number) => void;
    readonly jsrandomsource_new: (a: number, b: number) => void;
    readonly jstoolset_new: (a: number, b: number, c: number) => void;
    readonly normalizePrebetaShape: (a: number, b: number, c: number, d: number, e: number) => void;
    readonly runNoopTrace: (a: number) => void;
    readonly driveScriptedModelRequest: (a: number, b: number, c: number) => number;
    readonly driveScriptedToolCall: (a: number, b: number, c: number) => number;
    readonly driveScriptedJournalHealth: (a: number, b: number) => number;
    readonly __wbg_jsrandomsource_free: (a: number, b: number) => void;
    readonly __wasm_bindgen_func_elem_861: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_874: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_193: (a: number, b: number) => void;
    readonly __wbindgen_export: (a: number, b: number) => number;
    readonly __wbindgen_export2: (a: number, b: number, c: number, d: number) => number;
    readonly __wbindgen_export3: (a: number) => void;
    readonly __wbindgen_export4: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export5: (a: number, b: number, c: number) => void;
}

export type SyncInitInput = BufferSource | WebAssembly.Module;

/**
 * Instantiates the given `module`, which can either be bytes or
 * a precompiled `WebAssembly.Module`.
 *
 * @param {{ module: SyncInitInput }} module - Passing `SyncInitInput` directly is deprecated.
 *
 * @returns {InitOutput}
 */
export function initSync(module: { module: SyncInitInput } | SyncInitInput): InitOutput;

/**
 * If `module_or_path` is {RequestInfo} or {URL}, makes a request and
 * for everything else, calls `WebAssembly.instantiate` directly.
 *
 * @param {{ module_or_path: InitInput | Promise<InitInput> }} module_or_path - Passing `InitInput` directly is deprecated.
 *
 * @returns {Promise<InitOutput>}
 */
export default function __wbg_init (module_or_path?: { module_or_path: InitInput | Promise<InitInput> } | InitInput | Promise<InitInput>): Promise<InitOutput>;
