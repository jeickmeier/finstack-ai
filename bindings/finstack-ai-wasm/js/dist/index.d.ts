import type { PrebetaKind } from "./host.js";
export { Agent, Event, EventBatch, FinstackError, Lane, Locator, MemoryExternalIdentityMap, Run, RunResult, Session, } from "./agent.js";
export { POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, decodeFrame, decodeFrameLength, encodeFrame, } from "./remote.js";
export type { ActiveCapability, AgentOptions, Capability, CapabilityActivation, CapabilityCatalogItem, EventOptions, ExternalIdentitySnapshot, LaneInspectSnapshot, RunOptions, RunResultSnapshot, SessionInspectPhase, SessionInspectSnapshot, SessionSnapshot, } from "./agent.js";
export type { HostArtifactStore, HostCallOptions, HostClock, HostContextProvider, HostJournalStore, HostMiddleware, HostModel, HostModelCompletion, HostModelResult, HostObserver, HostRandomSource, HostToolResult, HostToolset, PrebetaKind, } from "./host.js";
export { JsArtifactStore, JsClock, JsContextProvider, JsJournalStore, JsMiddleware, JsModel, JsObserver, JsRandomSource, JsToolset, createHostClock, createHostRandomSource, createMemoryArtifactStore, createMemoryJournalStore, } from "./adapters.js";
export type { JsContextProviderOptions, JsJournalStoreOptions, JsMiddlewareOptions, JsModelOptions, JsObserverOptions, JsToolsetOptions, } from "./adapters.js";
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
 * Call this once before {@link health}, {@link buildMetadata}, or
 * {@link Agent.create}. The function does not create a Tokio runtime or open a
 * durable store. The host driver is installed when the generated module loads.
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
/**
 * Return payload digest, checksum, and canonical-CBOR hex from Rust.
 *
 * `kind` is `record_body` or `record_envelope`. JavaScript does not implement
 * a second CBOR codec.
 *
 * @param kind - Known-answer family.
 * @param value - Diagnostic JSON object of the body or envelope.
 * @returns Hex digests and canonical-CBOR text.
 * @throws {TypeError} When `kind` or `value` is invalid.
 * @example
 * ```ts
 * const answer = journalKnownAnswer("record_body", body);
 * ```
 */
export declare function journalKnownAnswer(kind: "record_body" | "record_envelope", value: unknown): {
    payload_digest: string;
    checksum?: string;
    cbor_hex: string;
};
/**
 * Normalize a pre-beta lineage or authenticated external-command shape.
 *
 * Durability-dependent store APIs remain pre-beta. This function only runs
 * Rust DTO validation and does not submit a live Agent.
 *
 * @param kind - One of the three supported command kinds.
 * @param value - Candidate JSON object.
 * @returns The Rust-normalized object.
 * @throws {TypeError} When `kind` is unsupported or `value` fails validation.
 * @example
 * ```ts
 * const normalized = normalizePrebetaShape("child_run_prepared", value);
 * ```
 */
export declare function normalizePrebetaShape(kind: PrebetaKind, value: unknown): unknown;
//# sourceMappingURL=index.d.ts.map