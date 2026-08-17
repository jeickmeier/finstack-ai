import type { HostModel } from "../host.js";
/** Options for the same-origin OpenAI Responses fetch/SSE battery. */
export interface OpenAIOptions {
    /**
     * Same-origin path or absolute URL for Responses.
     * Defaults to `/finstack/openai`.
     */
    baseUrl?: string;
    /** Optional application headers. Never a credential helper. */
    headers?: Record<string, string>;
    /** Optional model name override sent to the proxy. */
    model?: string;
}
/**
 * Same-origin default path for the official OpenAI Responses proxy.
 *
 * Browser bundles must not embed provider credentials. Point this path at a
 * trusted proxy that terminates secrets off-browser.
 */
export declare const OPENAI_DEFAULT_BASE_URL = "/finstack/openai";
/**
 * Create a {@link HostModel} that speaks OpenAI Responses SSE over `fetch`.
 *
 * The adapter sends `store: false`, treats `response.completed` as the only
 * successful terminal event, and propagates `AbortSignal`.
 */
export declare function createOpenAIModel(options?: OpenAIOptions): HostModel;
//# sourceMappingURL=openai.d.ts.map