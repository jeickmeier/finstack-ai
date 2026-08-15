import initWasm, { buildMetadata as wasmBuildMetadata, health as wasmHealth, journalKnownAnswer as wasmJournalKnownAnswer, normalizePrebetaShape as wasmNormalizePrebetaShape, } from "../generated/finstack_ai_wasm.js";
import { setAdaptersInitialized } from "./adapters.js";
export { Agent, Event, EventBatch, FinstackError, Lane, Locator, MemoryExternalIdentityMap, Run, RunResult, Session, } from "./agent.js";
export { POST_AUTH_FRAME_MAX_BYTES, PRE_AUTH_FRAME_MAX_BYTES, decodeFrame, decodeFrameLength, encodeFrame, } from "./remote.js";
export { JsArtifactStore, JsClock, JsContextProvider, JsJournalStore, JsMiddleware, JsModel, JsObserver, JsRandomSource, JsToolset, createHostClock, createHostRandomSource, createMemoryArtifactStore, createMemoryJournalStore, } from "./adapters.js";
let initialized = false;
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
export async function init(moduleOrPath) {
    if (initialized) {
        return;
    }
    await initWasm(moduleOrPath);
    initialized = true;
    setAdaptersInitialized(true);
}
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
export function health() {
    assertInitialized();
    return wasmHealth();
}
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
export function buildMetadata() {
    assertInitialized();
    const raw = wasmBuildMetadata();
    if (typeof raw.version !== "string" ||
        typeof raw.engineVersion !== "string" ||
        raw.implementation !== "wasm" ||
        raw.target !== "wasm32-unknown-unknown") {
        throw new Error("unexpected wasm build metadata");
    }
    return {
        version: raw.version,
        engineVersion: raw.engineVersion,
        implementation: "wasm",
        target: "wasm32-unknown-unknown",
    };
}
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
export function journalKnownAnswer(kind, value) {
    assertInitialized();
    switch (kind) {
        case "record_body":
        case "record_envelope":
            break;
        default: {
            const _exhaustive = kind;
            throw new TypeError(`unsupported known-answer kind: ${String(_exhaustive)}`);
        }
    }
    return JSON.parse(wasmJournalKnownAnswer(kind, JSON.stringify(value)));
}
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
export function normalizePrebetaShape(kind, value) {
    assertInitialized();
    switch (kind) {
        case "child_run_prepared":
        case "interaction_resolution":
        case "external_effect_completion":
            break;
        default: {
            const _exhaustive = kind;
            throw new TypeError(`unsupported pre-beta shape: ${String(_exhaustive)}`);
        }
    }
    const encoded = wasmNormalizePrebetaShape(kind, JSON.stringify(value));
    return JSON.parse(encoded);
}
function assertInitialized() {
    if (!initialized) {
        throw new Error("call init() before using @finstack/ai");
    }
}
//# sourceMappingURL=index.js.map