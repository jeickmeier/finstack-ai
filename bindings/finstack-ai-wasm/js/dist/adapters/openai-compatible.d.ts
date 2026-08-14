import type { HostModel } from "../host.js";
/**
 * Options for the same-origin OpenAI-compatible fetch/SSE battery.
 *
 * Application `headers` are optional. Do not embed provider credentials in
 * browser bundles; terminate secrets at a trusted same-origin proxy.
 */
export interface OpenAICompatibleOptions {
    /**
     * Same-origin path or absolute URL for chat completions.
     * Defaults to `/finstack/openai`.
     */
    baseUrl?: string;
    /** Optional application headers. Never a credential helper. */
    headers?: Record<string, string>;
    /** Optional model name override sent to the proxy. */
    model?: string;
}
/**
 * Same-origin default path for the OpenAI-compatible proxy.
 *
 * Browser bundles must not embed provider credentials. Point this path at a
 * trusted proxy that terminates secrets off-browser.
 */
export declare const OPENAI_COMPATIBLE_DEFAULT_BASE_URL = "/finstack/openai";
/**
 * Create a {@link HostModel} that speaks OpenAI-compatible SSE over `fetch`.
 *
 * The adapter owns no kernel semantics. CORS and network failures surface as
 * a stable `TypeError` (`js_host_failed`). `AbortSignal` is propagated.
 *
 * @param options - Same-origin URL, optional headers, and optional model name.
 * @returns A tree-shakeable host model implementation.
 * @throws When `fetch` fails or the response body is missing.
 * @example
 * ```ts
 * const model = createOpenAICompatibleModel();
 * const stream = await model.request(draft, { signal });
 * ```
 */
export declare function createOpenAICompatibleModel(options?: OpenAICompatibleOptions): HostModel;
//# sourceMappingURL=openai-compatible.d.ts.map