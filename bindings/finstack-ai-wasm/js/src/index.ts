import initWasm, {
  buildMetadata as wasmBuildMetadata,
  health as wasmHealth,
  journalKnownAnswer as wasmJournalKnownAnswer,
  normalizePrebetaShape as wasmNormalizePrebetaShape,
  parseDocument as wasmParseDocument,
  parseDocumentMarkdown as wasmParseDocumentMarkdown,
} from "../generated/finstack_ai_wasm.js";

import { setAdaptersInitialized } from "./adapters.js";
import type { PrebetaKind } from "./host.js";

export {
  Agent,
  ApprovalGrantMode,
  Event,
  EventBatch,
  FinstackError,
  HistoryCachePolicy,
  Lane,
  Locator,
  MemoryExternalIdentityMap,
  Run,
  RunResult,
  Session,
} from "./agent.js";
export {
  POST_AUTH_FRAME_MAX_BYTES,
  PRE_AUTH_FRAME_MAX_BYTES,
  decodeFrame,
  decodeFrameLength,
  encodeFrame,
} from "./remote.js";
export type {
  ActiveCapability,
  AgentOptions,
  Capability,
  CapabilityActivation,
  CapabilityCatalogItem,
  EventOptions,
  ExternalIdentitySnapshot,
  LaneInspectSnapshot,
  ObserverDiagnostic,
  ObserverDiagnostics,
  RunOptions,
  RunResultSnapshot,
  SessionInspectPhase,
  SessionInspectSnapshot,
  SessionSnapshot,
} from "./agent.js";

export type {
  HostArtifactStore,
  HostCallOptions,
  HostClock,
  HostContextProvider,
  HostJournalStore,
  HostMiddleware,
  HostModel,
  HostModelCompletion,
  HostModelResult,
  HostObserver,
  HostRandomSource,
  HostToolResult,
  HostToolset,
  PrebetaKind,
} from "./host.js";
export {
  JsArtifactStore,
  JsClock,
  JsContextProvider,
  JsJournalStore,
  JsMiddleware,
  JsModel,
  JsObserver,
  JsRandomSource,
  JsToolset,
  createHostClock,
  createHostRandomSource,
  createMemoryArtifactStore,
  createMemoryJournalStore,
} from "./adapters.js";
export type {
  JsContextProviderOptions,
  JsJournalStoreOptions,
  JsMiddlewareOptions,
  JsModelOptions,
  JsObserverOptions,
  JsToolsetOptions,
} from "./adapters.js";

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
export async function init(
  moduleOrPath?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module,
): Promise<void> {
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
export function health(): string {
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
export function buildMetadata(): BuildMetadata {
  assertInitialized();
  const raw = wasmBuildMetadata() as {
    version?: unknown;
    engineVersion?: unknown;
    implementation?: unknown;
    target?: unknown;
  };
  if (
    typeof raw.version !== "string" ||
    typeof raw.engineVersion !== "string" ||
    raw.implementation !== "wasm" ||
    raw.target !== "wasm32-unknown-unknown"
  ) {
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
export function journalKnownAnswer(
  kind: "record_body" | "record_envelope",
  value: unknown,
): { payload_digest: string; checksum?: string; cbor_hex: string } {
  assertInitialized();
  switch (kind) {
    case "record_body":
    case "record_envelope":
      break;
    default: {
      const _exhaustive: never = kind;
      throw new TypeError(`unsupported known-answer kind: ${String(_exhaustive)}`);
    }
  }
  return JSON.parse(wasmJournalKnownAnswer(kind, JSON.stringify(value))) as {
    payload_digest: string;
    checksum?: string;
    cbor_hex: string;
  };
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
export function normalizePrebetaShape(kind: PrebetaKind, value: unknown): unknown {
  assertInitialized();
  switch (kind) {
    case "child_run_prepared":
    case "interaction_resolution":
    case "external_effect_completion":
      break;
    default: {
      const _exhaustive: never = kind;
      throw new TypeError(`unsupported pre-beta shape: ${String(_exhaustive)}`);
    }
  }
  const encoded = wasmNormalizePrebetaShape(kind, JSON.stringify(value));
  return JSON.parse(encoded) as unknown;
}

/**
 * Detailed result of a debug document parse. See {@link parseDocument}.
 */
export interface ParsedDocument {
  /** GitHub-Flavored Markdown output (possibly truncated). */
  markdown: string;
  /** Detected format; the declared media type is only a hint. */
  format: string;
  /** Page count when the format has pages. */
  page_count: number | null;
  /** PDF page-content classification. */
  classification: string | null;
  /** Whether recovering full text needs OCR (scanned/image PDFs). */
  requires_ocr: boolean;
  /** Whether `markdown` was cut at the output byte ceiling. */
  truncated: boolean;
}

/**
 * Debug helper: parse a document and return only its Markdown.
 *
 * Runs the same `finstack-ai-tools-document` parser the document-ingest
 * middleware uses, without constructing an {@link Agent} or `Run`, so a
 * developer can see exactly what would be injected for a given file.
 *
 * @param data - Raw document bytes.
 * @param mediaType - Declared media type; a hint, not authoritative.
 * @returns The parsed GitHub-Flavored Markdown (possibly truncated).
 * @throws {TypeError} With a stable `document_*` error code when the input
 * is oversized, unsupported, or unparseable.
 * @example
 * ```ts
 * await init();
 * const markdown = parseDocumentMarkdown(bytes, "text/csv");
 * ```
 */
export function parseDocumentMarkdown(data: Uint8Array, mediaType: string): string {
  assertInitialized();
  return wasmParseDocumentMarkdown(data, mediaType);
}

/**
 * Debug helper: parse a document and return the full detailed result.
 *
 * @param data - Raw document bytes.
 * @param mediaType - Declared media type; a hint, not authoritative.
 * @returns Markdown plus format, page count, classification, OCR, and
 * truncation flags.
 * @throws {TypeError} With a stable `document_*` error code when the input
 * is oversized, unsupported, or unparseable.
 * @example
 * ```ts
 * await init();
 * const parsed = parseDocument(bytes, "application/pdf");
 * ```
 */
export function parseDocument(data: Uint8Array, mediaType: string): ParsedDocument {
  assertInitialized();
  return wasmParseDocument(data, mediaType) as ParsedDocument;
}

function assertInitialized(): void {
  if (!initialized) {
    throw new Error("call init() before using @finstack/ai");
  }
}
