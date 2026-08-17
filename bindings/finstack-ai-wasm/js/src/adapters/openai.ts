import type { HostCallOptions, HostModel, HostModelResult } from "../host.js";

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
export const OPENAI_DEFAULT_BASE_URL = "/finstack/openai";

/**
 * Create a {@link HostModel} that speaks OpenAI Responses SSE over `fetch`.
 *
 * The adapter sends `store: false`, treats `response.completed` as the only
 * successful terminal event, and propagates `AbortSignal`.
 */
export function createOpenAIModel(options: OpenAIOptions = {}): HostModel {
  const baseUrl = options.baseUrl ?? OPENAI_DEFAULT_BASE_URL;
  const headers = options.headers;
  const modelName = options.model;
  return {
    async request(draft: unknown, requestOptions?: HostCallOptions): Promise<HostModelResult> {
      const parsed = parseDraft(draft);
      const body = {
        model: modelName ?? readModel(parsed),
        input: readInput(parsed),
        stream: true,
        store: false,
      };
      const signal = detachAbort(requestOptions?.signal);
      let response: Response;
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
      } catch (error) {
        throw mapFetchError(error);
      }
      if (!response.ok || response.body === null) {
        throw new TypeError("js_host_failed");
      }
      return streamResponses(response.body, signal);
    },
  };
}

function detachAbort(signal?: AbortSignal): AbortSignal | undefined {
  if (signal === undefined) {
    return undefined;
  }
  const local = new AbortController();
  const abort = (): void => {
    setTimeout(() => {
      local.abort();
    }, 0);
  };
  if (signal.aborted) {
    abort();
  } else {
    signal.addEventListener("abort", abort, { once: true });
  }
  return local.signal;
}

function mapFetchError(error: unknown): Error {
  if (error instanceof DOMException && error.name === "AbortError") {
    return error;
  }
  if (error instanceof Error && error.name === "AbortError") {
    return error;
  }
  return new TypeError("js_host_failed");
}

function parseDraft(draft: unknown): unknown {
  if (typeof draft === "string") {
    try {
      return JSON.parse(draft) as unknown;
    } catch {
      return {};
    }
  }
  return draft;
}

function readModel(draft: unknown): string {
  if (typeof draft === "object" && draft !== null && "model" in draft) {
    const model = (draft as { model?: unknown }).model;
    if (typeof model === "string" && model.length > 0) {
      return model;
    }
  }
  return "scripted";
}

function readInput(
  draft: unknown,
): Array<{ role: string; content: Array<{ type: "input_text"; text: string }> }> {
  if (typeof draft !== "object" || draft === null || !("messages" in draft)) {
    return [{ role: "user", content: [{ type: "input_text", text: "hello" }] }];
  }
  const messages = (draft as { messages?: unknown }).messages;
  if (!Array.isArray(messages) || messages.length === 0) {
    return [{ role: "user", content: [{ type: "input_text", text: "hello" }] }];
  }
  return messages.map((message) => {
    if (typeof message !== "object" || message === null) {
      return { role: "user", content: [{ type: "input_text" as const, text: "hello" }] };
    }
    const role =
      "role" in message && typeof message.role === "string" ? message.role : "user";
    return {
      role,
      content: [{ type: "input_text" as const, text: extractText(message) }],
    };
  });
}

function extractText(message: object): string {
  if (!("content" in message)) {
    return "";
  }
  const content = (message as { content?: unknown }).content;
  if (typeof content === "string") {
    return content;
  }
  if (!Array.isArray(content)) {
    return "";
  }
  return content
    .map((block) => {
      if (typeof block === "object" && block !== null && "text" in block) {
        const text = (block as { text?: unknown }).text;
        return typeof text === "string" ? text : "";
      }
      return "";
    })
    .join("");
}

function streamResponses(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): ReadableStream<unknown> {
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
            if (
              event.type === "response.failed" ||
              event.type === "response.incomplete" ||
              event.type === "error"
            ) {
              throw new TypeError("js_host_failed");
            }
          }
        }
        throw new TypeError("js_host_failed");
      } catch (error) {
        reader.releaseLock();
        controller.error(mapFetchError(error));
      }
    },
    cancel() {
      void body.cancel();
    },
  });
}

function readSseData(frame: string): string | null {
  const data = frame
    .split("\n")
    .filter((line) => line.startsWith("data:"))
    .map((line) => line.slice(5).trim());
  return data.length === 0 ? null : data.join("\n");
}

type ResponsesEvent = {
  type?: unknown;
  delta?: unknown;
  response?: unknown;
};

function parseEvent(data: string): ResponsesEvent | null {
  try {
    return JSON.parse(data) as ResponsesEvent;
  } catch {
    return null;
  }
}

function readCompletionId(event: ResponsesEvent): string {
  if (typeof event.response === "object" && event.response !== null && "id" in event.response) {
    const id = (event.response as { id?: unknown }).id;
    if (typeof id === "string" && id.length > 0) {
      return id;
    }
  }
  throw new TypeError("js_host_failed");
}
