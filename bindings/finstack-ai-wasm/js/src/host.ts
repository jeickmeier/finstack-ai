/**
 * Trusted JavaScript host ABI for `@finstack/ai`.
 *
 * Host objects remain the contract (ADR-019). The wasm crate wraps them as
 * local `!Send` port implementations. IDs are strings; dynamic JSON crosses
 * the boundary as strings or `Uint8Array`.
 */

/**
 * Optional cancellation and call options passed to host methods.
 */
export interface HostCallOptions {
  /** Browser abort signal propagated into the host invoke. */
  signal?: AbortSignal;
}

/**
 * Completed coarse model object accepted by the Rust port wrapper.
 */
export interface HostModelCompletion {
  /** Assistant text. Mutually exclusive with {@link HostModelCompletion.json}. */
  text: string;
  /** Non-empty provider completion identity. */
  completion_id: string;
  /** Optional model-visible tool calls. */
  tool_calls?: unknown[];
  /** Structured JSON completion. Mutually exclusive with {@link HostModelCompletion.text}. */
  json?: unknown;
}

/**
 * A host model may return one completed object, a readable stream, or an async iterable.
 */
export type HostModelResult =
  | HostModelCompletion
  | ReadableStream<unknown>
  | AsyncIterable<unknown>;

/**
 * Trusted model host. No per-token callback is required.
 */
export interface HostModel {
  /**
   * Fulfill one committed model request.
   *
   * @param draft - JSON string of the committed model draft.
   * @param options - Optional {@link AbortSignal}.
   * @returns A completed object, `ReadableStream`, or async iterable.
   */
  request(draft: unknown, options?: HostCallOptions): Promise<HostModelResult>;
}

/**
 * Completed coarse tool object accepted by the Rust port wrapper.
 */
export interface HostToolResult {
  /** Canonical JSON object or array output. */
  output: object;
  /** Application-level error flag. */
  is_error?: boolean;
}

/**
 * Trusted toolset host.
 */
export interface HostToolset {
  /**
   * Fulfill one validated tool call.
   *
   * @param context - JSON string of the call locator.
   * @param call - JSON string of the validated tool call.
   * @param options - Optional {@link AbortSignal}.
   */
  call(
    context: unknown,
    call: unknown,
    options?: HostCallOptions,
  ): Promise<HostToolResult | ReadableStream<unknown> | AsyncIterable<unknown>>;
}

/**
 * Trusted context-provider host.
 */
export interface HostContextProvider {
  /**
   * Collect one context contribution.
   *
   * @param request - JSON string of the context request.
   * @param options - Optional {@link AbortSignal}.
   */
  collect(request: unknown, options?: HostCallOptions): Promise<unknown>;
}

/**
 * Trusted middleware host.
 */
export interface HostMiddleware {
  /**
   * Invoke one stage.
   *
   * @param input - JSON string of the stage input.
   * @param options - Optional {@link AbortSignal}.
   */
  invoke(input: unknown, options?: HostCallOptions): Promise<unknown>;
}

/**
 * Trusted observer host. Receives a projected event batch, not per-token hooks.
 */
export interface HostObserver {
  /**
   * Observe one event batch.
   *
   * @param batch - JSON string of the projected batch.
   * @param options - Optional {@link AbortSignal}.
   */
  observe(batch: unknown, options?: HostCallOptions): Promise<void>;
}

/**
 * Trusted host journal store.
 *
 * `health` is required. `append`, `load`, and `writeSnapshot` are optional;
 * the Rust wrapper returns Unavailable when they are missing.
 * `durable` is always treated as false by the wrapper.
 */
export interface HostJournalStore {
  /**
   * Report readiness. `durable` is always treated as false by the wrapper.
   */
  health(options?: HostCallOptions): Promise<{
    ready: boolean;
    detail?: string;
  }>;
  /**
   * Append one frozen transitional JSON batch.
   *
   * @param request - JSON string of `AppendRequest`.
   */
  append?(request: unknown, options?: HostCallOptions): Promise<unknown>;
  /**
   * Load one session's committed batches.
   *
   * @param request - JSON string of `{ session_id }`.
   */
  load?(request: unknown, options?: HostCallOptions): Promise<unknown>;
  /**
   * Replace one session's disposable snapshot cache.
   *
   * @param request - JSON string of `SnapshotRequest`.
   */
  writeSnapshot?(request: unknown, options?: HostCallOptions): Promise<unknown>;
}

/**
 * Host clock. `now` returns Unix milliseconds.
 */
export interface HostClock {
  /** Current Unix time in milliseconds. */
  now(): number;
}

/**
 * Host entropy source.
 */
export interface HostRandomSource {
  /**
   * Fill `length` random bytes.
   *
   * @param length - Number of bytes to produce.
   */
  fillBytes(length: number): Uint8Array;
}

/**
 * Host artifact store. Rust owns `ArtifactRef` construction.
 */
export interface HostArtifactStore {
  /**
   * Persist exact bytes before the referencing journal append.
   *
   * @param scope - JSON string of the artifact scope.
   * @param content - Exact bytes.
   * @param metadata - JSON string of artifact metadata.
   * @param id - Artifact identity string assigned by Rust.
   */
  stagePut(
    scope: unknown,
    content: Uint8Array,
    metadata: unknown,
    id?: unknown,
  ): Promise<void>;
  /**
   * Read exact bytes for a previously staged artifact.
   *
   * @param key - Artifact identity string.
   */
  get(key: unknown): Promise<Uint8Array>;
}

/**
 * Supported pre-beta command kinds.
 */
export type PrebetaKind =
  | "child_run_prepared"
  | "interaction_resolution"
  | "external_effect_completion";
