/**
 * Same-origin default path for the official OpenAI Responses proxy.
 *
 * Browser bundles must not embed provider credentials. Point this path at a
 * trusted proxy that terminates secrets off-browser.
 */
export const OPENAI_DEFAULT_BASE_URL = "/finstack/openai";
/**
 * Create a {@link HostModel} that speaks OpenAI Responses SSE over `fetch`.
 *
 * The adapter sends `store: false`, treats `response.completed` as the only
 * successful terminal event, and propagates `AbortSignal`.
 */
export function createOpenAIModel(options = {}) {
    const baseUrl = options.baseUrl ?? OPENAI_DEFAULT_BASE_URL;
    const headers = options.headers;
    const modelName = options.model;
    return {
        async request(draft, requestOptions) {
            const parsed = parseDraft(draft);
            const body = {
                model: modelName ?? readModel(parsed),
                input: readInput(parsed),
                stream: true,
                store: false,
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
            return streamResponses(response.body, signal);
        },
    };
}
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
function readInput(draft) {
    if (typeof draft !== "object" || draft === null || !("messages" in draft)) {
        return [{ role: "user", content: [{ type: "input_text", text: "hello" }] }];
    }
    const messages = draft.messages;
    if (!Array.isArray(messages) || messages.length === 0) {
        return [{ role: "user", content: [{ type: "input_text", text: "hello" }] }];
    }
    return messages.map((message) => {
        if (typeof message !== "object" || message === null) {
            return { role: "user", content: [{ type: "input_text", text: "hello" }] };
        }
        const role = "role" in message && typeof message.role === "string" ? message.role : "user";
        return {
            role,
            content: [{ type: "input_text", text: extractText(message) }],
        };
    });
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
function streamResponses(body, signal) {
    return new ReadableStream({
        async start(controller) {
            const reader = body.getReader();
            const decoder = new TextDecoder();
            let buffer = "";
            let text = "";
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
                        if (data === null || data === "[DONE]") {
                            continue;
                        }
                        const event = parseEvent(data);
                        if (event === null) {
                            continue;
                        }
                        if (event.type === "response.output_text.delta") {
                            const delta = typeof event.delta === "string" ? event.delta : "";
                            if (delta.length > 0) {
                                text += delta;
                                controller.enqueue({ text: delta });
                            }
                            continue;
                        }
                        if (event.type === "response.completed") {
                            const completionId = readCompletionId(event);
                            controller.enqueue({ text, completion_id: completionId });
                            controller.close();
                            return;
                        }
                        if (event.type === "response.failed" ||
                            event.type === "response.incomplete" ||
                            event.type === "error") {
                            throw new TypeError("js_host_failed");
                        }
                    }
                }
                throw new TypeError("js_host_failed");
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
    const data = frame
        .split("\n")
        .filter((line) => line.startsWith("data:"))
        .map((line) => line.slice(5).trim());
    return data.length === 0 ? null : data.join("\n");
}
function parseEvent(data) {
    try {
        return JSON.parse(data);
    }
    catch {
        return null;
    }
}
function readCompletionId(event) {
    if (typeof event.response === "object" && event.response !== null && "id" in event.response) {
        const id = event.response.id;
        if (typeof id === "string" && id.length > 0) {
            return id;
        }
    }
    throw new TypeError("js_host_failed");
}
//# sourceMappingURL=openai.js.map