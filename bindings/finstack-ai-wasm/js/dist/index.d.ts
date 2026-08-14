/**
 * Lockstep version metadata for the published `@finstack/ai` package.
 */
export interface BuildMetadata {
    /** Package version string. */
    version: string;
    /** Engine version string; lockstep with {@link BuildMetadata.version}. */
    engineVersion: string;
    /** Binding implementation identity. */
    implementation: "wasm";
    /** Published compilation target. */
    target: "wasm32-unknown-unknown";
}
/**
 * Load the generated wasm-bindgen module.
 *
 * Call this once before {@link health} or {@link buildMetadata}. The function
 * does not create a Tokio runtime, open a store, or spawn host work.
 *
 * @param moduleOrPath - Optional wasm module, URL, or bytes. Defaults to the
 * generated `finstack_ai_wasm_bg.wasm` next to the glue.
 * @returns A promise that resolves when the engine module is ready.
 * @example
 * ```ts
 * await init();
 * health(); // "ok"
 * ```
 */
export declare function init(moduleOrPath?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module): Promise<void>;
/**
 * Return the process-local health token.
 *
 * @returns `"ok"` after {@link init}.
 * @throws When {@link init} has not completed.
 * @example
 * ```ts
 * await init();
 * const status = health();
 * ```
 */
export declare function health(): string;
/**
 * Return lockstep version metadata for this wasm package.
 *
 * @returns Metadata whose `implementation` is `"wasm"`.
 * @throws When {@link init} has not completed or metadata is malformed.
 * @example
 * ```ts
 * await init();
 * const metadata = buildMetadata();
 * ```
 */
export declare function buildMetadata(): BuildMetadata;
//# sourceMappingURL=index.d.ts.map