import { Agent, JsJournalStore, JsModel, init } from "@finstack/ai";
import { createIndexedDbJournalStore } from "@finstack/ai/adapters/indexeddb";
import { createOpenAICompatibleModel } from "@finstack/ai/adapters/openai-compatible";
import { exposeWorkerHost } from "@finstack/ai/worker";

const MODEL_OPTIONS = {
  component: "js.model.browser-minimal",
  provider: "js-demo",
  model: "js-demo-model",
};

const wasmReady = init();
let store: JsJournalStore | undefined;

exposeWorkerHost({
  async inspectSession(sessionId) {
    await wasmReady;
    return Agent.inspectSession(journalStore(), sessionId);
  },
  async create(options) {
    await wasmReady;
    const scenario =
      options !== null &&
      typeof options === "object" &&
      "scenario" in options &&
      (options as { scenario?: unknown }).scenario === "openai"
        ? "openai"
        : "scripted";
    if (scenario === "openai") {
      return Agent.create({
        model: new JsModel(createOpenAICompatibleModel(), MODEL_OPTIONS),
        store: journalStore(),
      });
    }
    return Agent.create({
      model: new JsModel(
        {
          request: async () => ({
            text: "hello from browser-minimal",
            completion_id: "browser-minimal-1",
          }),
        },
        MODEL_OPTIONS,
      ),
      store: journalStore(),
    });
  },
});

function journalStore(): JsJournalStore {
  store ??= new JsJournalStore(createIndexedDbJournalStore(), {
    detail: "js_indexeddb_experimental",
  });
  return store;
}
