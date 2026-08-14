/**
 * Same-origin default path for the OpenAI-compatible proxy.
 *
 * Browser bundles must not embed provider credentials. Point this path at a
 * trusted proxy that terminates secrets off-browser.
 */
export const OPENAI_COMPATIBLE_DEFAULT_BASE_URL = "/finstack/openai";
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
export function createOpenAICompatibleModel(options = {}) {
    const baseUrl = options.baseUrl ?? OPENAI_COMPATIBLE_DEFAULT_BASE_URL;
    const headers = options.headers;
    const modelName = options.model;
    return {
        async request(draft, requestOptions) {
            const parsed = parseDraft(draft);
            const body = {
                model: modelName ?? readModel(parsed),
                messages: readMessages(parsed),
                stream: true,
            };
            const signal = detachAbort(requestOptions?.signal);
            let response;
            try {
                response = await fetch(baseUrl, {
                    method: "POST",
                    headers: {
                        accept: "text/event-stream",
                        "content-type": "application/json",
                        ...(headers ?? {}),
                    },
                    body: JSON.stringify(body),
                    ...(signal === undefined ? {} : { signal }),
                });
            }
            catch (error) {
                throw mapFetchError(error);
            }
            if (!response.ok || response.body === null) {
                throw new TypeError("js_host_failed");
            }
            return streamSse(response.body, signal);
        },
    };
}
/**
 * Copy an incoming abort onto a local controller after the current turn.
 *
 * wasm-bindgen drops host futures during `Run.cancel`. Attaching that signal
 * directly to `fetch` re-enters a dropped closure. A detached controller
 * aborts the network after cancel returns.
 */
function detachAbort(signal) {
    if (signal === undefined) {
        return undefined;
    }
    const local = new AbortController();
    const abort = () => {
        setTimeout(() => {
            local.abort();
        }, 0);
    };
    if (signal.aborted) {
        abort();
    }
    else {
        signal.addEventListener("abort", abort, { once: true });
    }
    return local.signal;
}
function mapFetchError(error) {
    if (error instanceof DOMException && error.name === "AbortError") {
        return error;
    }
    if (error instanceof Error && error.name === "AbortError") {
        return error;
    }
    return new TypeError("js_host_failed");
}
function parseDraft(draft) {
    if (typeof draft === "string") {
        try {
            return JSON.parse(draft);
        }
        catch {
            return {};
        }
    }
    return draft;
}
function readModel(draft) {
    if (typeof draft === "object" && draft !== null && "model" in draft) {
        const model = draft.model;
        if (typeof model === "string" && model.length > 0) {
            return model;
        }
    }
    return "scripted";
}
function readMessages(draft) {
    if (typeof draft === "object" && draft !== null && "messages" in draft) {
        const messages = draft.messages;
        if (Array.isArray(messages) && messages.length > 0) {
            return messages.map((message) => {
                if (typeof message !== "object" || message === null) {
                    return { role: "user", content: "hello" };
                }
                const role = "role" in message && typeof message.role === "string" ? message.role : "user";
                return { role, content: extractText(message) };
            });
        }
    }
    return [{ role: "user", content: "hello" }];
}
function extractText(message) {
    if (!("content" in message)) {
        return "";
    }
    const content = message.content;
    if (typeof content === "string") {
        return content;
    }
    if (!Array.isArray(content)) {
        return "";
    }
    return content
        .map((block) => {
        if (typeof block === "object" && block !== null && "text" in block) {
            const text = block.text;
            return typeof text === "string" ? text : "";
        }
        return "";
    })
        .join("");
}
function streamSse(body, signal) {
    return new ReadableStream({
        async start(controller) {
            const reader = body.getReader();
            const decoder = new TextDecoder();
            let buffer = "";
            let text = "";
            let completionId = "openai-compatible";
            try {
                while (true) {
                    if (signal?.aborted) {
                        throw new DOMException("Aborted", "AbortError");
                    }
                    const { done, value } = await reader.read();
                    if (done) {
                        break;
                    }
                    buffer += decoder.decode(value, { stream: true });
                    const frames = buffer.split("\n\n");
                    buffer = frames.pop() ?? "";
                    for (const frame of frames) {
                        const data = readSseData(frame);
                        if (data === null) {
                            continue;
                        }
                        if (data === "[DONE]") {
                            controller.enqueue({
                                text,
                                completion_id: completionId,
                            });
                            controller.close();
                            return;
                        }
                        const parsed = parseSseJson(data);
                        if (parsed === null) {
                            continue;
                        }
                        if (typeof parsed.id === "string" && parsed.id.length > 0) {
                            completionId = parsed.id;
                        }
                        const delta = readDeltaText(parsed);
                        if (delta.length > 0) {
                            text += delta;
                            controller.enqueue({ text: delta });
                        }
                    }
                }
                controller.enqueue({
                    text,
                    completion_id: completionId,
                });
                controller.close();
            }
            catch (error) {
                reader.releaseLock();
                controller.error(mapFetchError(error));
            }
        },
        cancel() {
            void body.cancel();
        },
    });
}
function readSseData(frame) {
    const lines = frame.split("\n");
    const data = [];
    for (const line of lines) {
        if (line.startsWith("data:")) {
            data.push(line.slice(5).trim());
        }
    }
    if (data.length === 0) {
        return null;
    }
    return data.join("\n");
}
function parseSseJson(data) {
    try {
        return JSON.parse(data);
    }
    catch {
        return null;
    }
}
function readDeltaText(parsed) {
    if (!Array.isArray(parsed.choices) || parsed.choices[0] === undefined) {
        return "";
    }
    const choice = parsed.choices[0];
    if (typeof choice !== "object" || choice === null || !("delta" in choice)) {
        return "";
    }
    const delta = choice.delta;
    if (typeof delta !== "object" || delta === null || !("content" in delta)) {
        return "";
    }
    const content = delta.content;
    return typeof content === "string" ? content : "";
}
//# sourceMappingURL=openai-compatible.js.map