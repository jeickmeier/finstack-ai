/**
 * Worker for the browser knowledge example.
 *
 * The distinct job of this example: a **TypeScript-implemented retrieval
 * toolset** registered through the host adapter surface (`JsToolset`),
 * searching an in-page corpus. The scripted scenario needs no provider and
 * no network; the journal persists in IndexedDB.
 */

import { Agent, JsJournalStore, JsModel, JsToolset, init } from "@finstack/ai";
import type { HostModel, HostModelResult } from "@finstack/ai";
import { createIndexedDbJournalStore } from "@finstack/ai/adapters/indexeddb";
import { exposeWorkerHost } from "@finstack/ai/worker";

import type * as Retrieval from "./retrieval.mjs";

// `retrieval.mjs` is browser-executable source served beside this file's
// compiled `dist/` output, so it is loaded by URL (one module serves the
// worker, the page, and the Playwright spec); the static import above is
// type-only.
const retrievalReady: Promise<typeof Retrieval> = import(
  new URL("../retrieval.mjs", import.meta.url).href
);

const MODEL_OPTIONS = {
  component: "know.model.browser",
  provider: "js-knowledge",
  model: "js-knowledge-model",
};

const INSTRUCTION =
  "You are the finstack knowledge assistant. Search the corpus with " +
  "search_corpus when a question touches it, and cite doc ids in answers.";

const wasmReady = init();
let store: JsJournalStore | undefined;

interface CreateOptions {
  /** Scripted model turns: each is a final text or a search call + text. */
  script?: ScriptedTurn[];
}

type ScriptedTurn =
  | { text: string }
  | { search: string; text_template: string };

exposeWorkerHost({
  async inspectSession(sessionId) {
    await wasmReady;
    return Agent.inspectSession(journalStore(), sessionId);
  },
  async create(options) {
    await wasmReady;
    const { CORPUS, RETRIEVAL_TOOLSET_OPTIONS, createRetrievalHost } = await retrievalReady;
    const script = readScript(options);
    return Agent.create({
      model: new JsModel(scriptedModel(script), MODEL_OPTIONS),
      store: journalStore(),
      toolsets: [
        new JsToolset(createRetrievalHost(CORPUS), RETRIEVAL_TOOLSET_OPTIONS),
      ],
      instruction: INSTRUCTION,
    });
  },
});

/**
 * A deterministic scripted model. A `{search, text_template}` turn first
 * issues a `search_corpus` call, then answers with the template's
 * `{excerpt}`/`{doc_id}` placeholders filled from the tool result it sees
 * in the follow-up request — proving the excerpt actually flowed through
 * the run, not through this script.
 */
function scriptedModel(script: ScriptedTurn[]): HostModel {
  let turn = 0;
  let pending: { text_template: string } | undefined;
  let completion = 0;
  return {
    request: async (request: unknown): Promise<HostModelResult> => {
      completion += 1;
      if (pending !== undefined) {
        const { text_template } = pending;
        pending = undefined;
        const excerpt = latestExcerpt(request);
        return {
          text: text_template
            .replace("{excerpt}", excerpt?.excerpt ?? "")
            .replace("{doc_id}", excerpt?.doc_id ?? ""),
          completion_id: `know-${completion}`,
        };
      }
      const entry = script[Math.min(turn, script.length - 1)] ?? {
        text: "I do not know: the script is empty.",
      };
      turn += 1;
      if ("search" in entry) {
        pending = { text_template: entry.text_template };
        return {
          text: "",
          completion_id: `know-${completion}`,
          tool_calls: [
            {
              name: "search_corpus",
              arguments: { query: entry.search, top_k: 2 },
            },
          ],
        };
      }
      return { text: entry.text, completion_id: `know-${completion}` };
    },
  };
}

/** Pull the top excerpt out of the latest tool-result block, if any. */
function latestExcerpt(
  request: unknown,
): { excerpt: string; doc_id: string } | undefined {
  // The committed model draft arrives as a JSON string.
  const draft: unknown =
    typeof request === "string" ? JSON.parse(request) : request;
  const messages =
    typeof draft === "object" && draft !== null && "messages" in draft
      ? (draft as { messages: unknown[] }).messages
      : [];
  for (let index = messages.length - 1; index >= 0; index -= 1) {
    const message = messages[index] as { content?: unknown[] } | undefined;
    for (const block of message?.content ?? []) {
      const value = (block as { content?: { value?: unknown }[] }).content;
      const output = value?.[0]?.value as
        | { excerpts?: { excerpt: string; doc_id: string }[] }
        | undefined;
      const top = output?.excerpts?.[0];
      if (top !== undefined) {
        return top;
      }
    }
  }
  return undefined;
}

function readScript(options: unknown): ScriptedTurn[] {
  if (
    options !== null &&
    typeof options === "object" &&
    "script" in options &&
    Array.isArray((options as CreateOptions).script)
  ) {
    return (options as CreateOptions).script ?? [];
  }
  return [
    {
      search: "runtime ports",
      text_template: "From the {doc_id} doc: {excerpt}",
    },
  ];
}

function journalStore(): JsJournalStore {
  store ??= new JsJournalStore(createIndexedDbJournalStore(), {
    detail: "js_indexeddb_experimental",
  });
  return store;
}
