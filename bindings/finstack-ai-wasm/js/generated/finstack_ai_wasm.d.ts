/* tslint:disable */
/* eslint-disable */

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
 * Process-local health token. Does not create a runtime, open a store, or spawn work.
 */
export function health(): string;

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
    readonly buildMetadata: (a: number) => void;
    readonly compilePortProxies: () => void;
    readonly health: (a: number) => void;
    readonly runNoopTrace: (a: number) => void;
    readonly __wasm_bindgen_func_elem_403: (a: number, b: number, c: number, d: number) => void;
    readonly __wasm_bindgen_func_elem_416: (a: number, b: number, c: number, d: number) => void;
    readonly __wbindgen_export: (a: number) => void;
    readonly __wbindgen_export2: (a: number, b: number) => void;
    readonly __wbindgen_add_to_stack_pointer: (a: number) => number;
    readonly __wbindgen_export3: (a: number, b: number, c: number) => void;
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
