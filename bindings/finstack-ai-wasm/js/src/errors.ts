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
  /**
   * Optional model-activated capability id.
   * Omitted or `undefined` runs this agent; a missing catalog id fails closed.
   */
  capability?: string;
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
export class FinstackError extends Error {
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
  constructor(
    message: string,
    options: {
      code: string;
      retryable: boolean;
      context?: SessionSnapshot;
    },
  ) {
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
  static fromUnknown(error: unknown): FinstackError {
    if (error instanceof FinstackError) {
      return error;
    }
    if (error instanceof Error) {
      const record = error as Error & {
        code?: unknown;
        retryable?: unknown;
        context?: unknown;
      };
      const options: {
        code: string;
        retryable: boolean;
        context?: SessionSnapshot;
      } = {
        code:
          typeof record.code === "string"
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
] as const;

function codeFromMessage(message: string): string | undefined {
  return STABLE_CODES.find(
    (code) => message === code || message.startsWith(`${code}:`),
  );
}

/**
 * Parse a session locator from an unknown protocol or error payload.
 *
 * @param value - Candidate locator object.
 * @returns A snapshot when all four identity fields are strings.
 */
export function sessionSnapshot(value: unknown): SessionSnapshot | undefined {
  if (value === null || typeof value !== "object") {
    return undefined;
  }
  const record = value as Record<string, unknown>;
  if (
    typeof record.tenantScope !== "string" ||
    typeof record.sessionId !== "string" ||
    typeof record.laneId !== "string" ||
    typeof record.runId !== "string"
  ) {
    return undefined;
  }
  return {
    tenantScope: record.tenantScope,
    sessionId: record.sessionId,
    laneId: record.laneId,
    runId: record.runId,
  };
}
