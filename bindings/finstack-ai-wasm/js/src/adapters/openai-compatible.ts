import type { HostCallOptions, HostModel, HostModelResult } from "../host.js";

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
export function createOpenAICompatibleModel(
  options: OpenAICompatibleOptions = {},
): HostModel {
  const baseUrl = options.baseUrl ?? OPENAI_COMPATIBLE_DEFAULT_BASE_URL;
  const headers = options.headers;
  const modelName = options.model;
  return {
    async request(draft: unknown, requestOptions?: HostCallOptions): Promise<HostModelResult> {
      const parsed = parseDraft(draft);
      const body = {
        model: modelName ?? readModel(parsed),
        messages: readMessages(parsed),
        stream: true,
      };
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
          ...(requestOptions?.signal ? { signal: requestOptions.signal } : {}),
        });
      } catch (error) {
        throw mapFetchError(error);
      }
      if (!response.ok || response.body === null) {
        throw new TypeError("js_host_failed");
      }
      return streamSse(response.body, requestOptions?.signal);
    },
  };
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

function readMessages(draft: unknown): Array<{ role: string; content: string }> {
  if (typeof draft === "object" && draft !== null && "messages" in draft) {
    const messages = (draft as { messages?: unknown }).messages;
    if (Array.isArray(messages) && messages.length > 0) {
      return messages.map((message) => {
        if (typeof message !== "object" || message === null) {
          return { role: "user", content: "hello" };
        }
        const role =
          "role" in message && typeof message.role === "string" ? message.role : "user";
        return { role, content: extractText(message) };
      });
    }
  }
  return [{ role: "user", content: "hello" }];
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

function streamSse(
  body: ReadableStream<Uint8Array>,
  signal?: AbortSignal,
): ReadableStream<unknown> {
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
  const lines = frame.split("\n");
  const data: string[] = [];
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

function parseSseJson(data: string): { id?: unknown; choices?: unknown } | null {
  try {
    return JSON.parse(data) as { id?: unknown; choices?: unknown };
  } catch {
    return null;
  }
}

function readDeltaText(parsed: { choices?: unknown }): string {
  if (!Array.isArray(parsed.choices) || parsed.choices[0] === undefined) {
    return "";
  }
  const choice = parsed.choices[0];
  if (typeof choice !== "object" || choice === null || !("delta" in choice)) {
    return "";
  }
  const delta = (choice as { delta?: unknown }).delta;
  if (typeof delta !== "object" || delta === null || !("content" in delta)) {
    return "";
  }
  const content = (delta as { content?: unknown }).content;
  return typeof content === "string" ? content : "";
}
