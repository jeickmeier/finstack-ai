/**
 * Structured engine error with a stable code.
 */
export class FinstackError extends Error {
    /** Stable machine-readable code. */
    code;
    /** Whether an identical frontend call is safe to retry automatically. */
    retryable;
    /** Optional session locator captured at failure. */
    context;
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
    constructor(message, options) {
        super(message);
        this.name = "FinstackError";
        this.code = options.code;
        this.retryable = options.retryable;
        if (options.context !== undefined) {
            this.context = options.context;
        }
    }
    /**
     * Wrap a thrown wasm or host value as {@link FinstackError}.
     *
     * @param error - Unknown rejection or throw value.
     * @returns A structured error. Already-wrapped values are returned as-is.
     */
    static fromUnknown(error) {
        if (error instanceof FinstackError) {
            return error;
        }
        if (error instanceof Error) {
            const record = error;
            const options = {
                code: typeof record.code === "string"
                    ? record.code
                    : codeFromMessage(error.message) ?? "agent_run_runtime_failure",
                retryable: record.retryable === true,
            };
            const context = sessionSnapshot(record.context);
            if (context !== undefined) {
                options.context = context;
            }
            return new FinstackError(error.message, options);
        }
        return new FinstackError(String(error), {
            code: "agent_run_runtime_failure",
            retryable: false,
        });
    }
}
const STABLE_CODES = [
    "agent_run_invalid_configuration",
    "agent_run_timeout",
    "agent_run_cancelled",
    "agent_run_runtime_failure",
    "agent_run_unsupported_plan",
];
function codeFromMessage(message) {
    return STABLE_CODES.find((code) => message === code || message.startsWith(`${code}:`));
}
/**
 * Parse a session locator from an unknown protocol or error payload.
 *
 * @param value - Candidate locator object.
 * @returns A snapshot when all four identity fields are strings.
 */
export function sessionSnapshot(value) {
    if (value === null || typeof value !== "object") {
        return undefined;
    }
    const record = value;
    if (typeof record.tenantScope !== "string" ||
        typeof record.sessionId !== "string" ||
        typeof record.laneId !== "string" ||
        typeof record.runId !== "string") {
        return undefined;
    }
    return {
        tenantScope: record.tenantScope,
        sessionId: record.sessionId,
        laneId: record.laneId,
        runId: record.runId,
    };
}
//# sourceMappingURL=errors.js.map