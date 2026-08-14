/**
 * Options for {@link Agent.start} and {@link Agent.run}.
 */
export interface RunOptions {
    /** Operational deadline in seconds. Defaults to 30; maximum 86400. */
    timeoutSeconds?: number;
    /** Maximum model cycles. Defaults to 16. */
    maxCycles?: number;
    /** Maximum structured-output retries. Defaults to 1. */
    maxOutputRetries?: number;
}
/**
 * Options for {@link Run.events}.
 */
export interface EventOptions {
    /**
     * Cancels iteration and closes event delivery only.
     * Does not cancel the durable run.
     */
    signal?: AbortSignal;
}
/**
 * Explicit session locator snapshot.
 */
export interface SessionSnapshot {
    /** Tenant scope captured at acceptance. */
    tenantScope: string;
    /** Session identity. */
    sessionId: string;
    /** Lane identity. */
    laneId: string;
    /** Run identity. */
    runId: string;
}
/**
 * Explicit successful-result snapshot.
 */
export interface RunResultSnapshot extends SessionSnapshot {
    /** Concatenated final assistant text. */
    text: string;
}
/**
 * Structured engine error with a stable code.
 */
export declare class FinstackError extends Error {
    /** Stable machine-readable code. */
    readonly code: string;
    /** Whether an identical frontend call is safe to retry automatically. */
    readonly retryable: boolean;
    /** Optional session locator captured at failure. */
    readonly context?: SessionSnapshot;
    /**
     * Construct a structured engine error.
     *
     * @param message - Non-secret explanation.
     * @param options - Stable code, retryability, and optional session context.
     * @example
     * ```ts
     * throw new FinstackError("run was cancelled", {
     *   code: "agent_run_cancelled",
     *   retryable: false,
     * });
     * ```
     */
    constructor(message: string, options: {
        code: string;
        retryable: boolean;
        context?: SessionSnapshot;
    });
    /**
     * Wrap a thrown wasm or host value as {@link FinstackError}.
     *
     * @param error - Unknown rejection or throw value.
     * @returns A structured error. Already-wrapped values are returned as-is.
     */
    static fromUnknown(error: unknown): FinstackError;
}
/**
 * Parse a session locator from an unknown protocol or error payload.
 *
 * @param value - Candidate locator object.
 * @returns A snapshot when all four identity fields are strings.
 */
export declare function sessionSnapshot(value: unknown): SessionSnapshot | undefined;
//# sourceMappingURL=errors.d.ts.map