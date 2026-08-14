import initWasm, {
  buildMetadata as wasmBuildMetadata,
  health as wasmHealth,
} from "../generated/finstack_ai_wasm.js";

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
export async function init(
  moduleOrPath?: RequestInfo | URL | Response | BufferSource | WebAssembly.Module,
): Promise<void> {
  if (initialized) {
    return;
  }
  await initWasm(moduleOrPath);
  initialized = true;
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

function assertInitialized(): void {
  if (!initialized) {
    throw new Error("call init() before using @finstack/ai");
  }
}
