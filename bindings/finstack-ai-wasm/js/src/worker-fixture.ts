import { Agent, JsJournalStore, JsModel, JsToolset, init } from "./index.js";
import { createIndexedDbJournalStore } from "./adapters/indexeddb.js";
import { createOpenAIModel } from "./adapters/openai.js";
import { exposeWorkerHost } from "./worker-host.js";

const MODEL_OPTIONS = {
  component: "js.model.fixture",
  provider: "js-fixture",
  model: "js-fixture-model",
};

const ECHO_TOOL = {
  id: "js.echo",
  model_name: "echo",
  title: "Echo",
  description: "Echo one validated string value.",
  input_schema: {
    additionalProperties: false,
    properties: { value: { type: "string" } },
    required: ["value"],
    type: "object",
  },
  output_schema: {
    additionalProperties: false,
    properties: { value: { type: "string" } },
    required: ["value"],
    type: "object",
  },
  execution: "sequential",
  side_effect: "read_only",
  retry_safety: "safe_to_retry",
  approval: {
    requirement: "not_required",
    reason: null,
    attributes: {},
  },
  max_result_bytes: 4096,
  metadata: {},
};

const TOOL_OPTIONS = {
  component: "js.toolset.fixture",
  name: "js-fixture-tools",
  tools: [ECHO_TOOL],
};

type Scenario =
  | "model-only"
  | "tool-cycle"
  | "thousand-chunk"
  | "hanging-model"
  | "hanging-fetch"
  | "persist"
  | "hanging-persist";

const wasmReady = init();
let persistStore: JsJournalStore | undefined;
let persistDbName = "finstack-ai-experimental";

exposeWorkerHost({
  async inspectSession(sessionId) {
    await wasmReady;
    const store =
      persistStore ??
      new JsJournalStore(createIndexedDbJournalStore({ dbName: persistDbName }), {
        detail: "js_indexeddb_experimental",
      });
    persistStore = store;
    return Agent.inspectSession(store, sessionId);
  },
  async create(options) {
    await wasmReady;
    const scenario = readScenario(options);
    switch (scenario) {
      case "model-only":
        return Agent.create({
          model: new JsModel(
            {
              request: async () => ({
                text: "hello from Agent",
                completion_id: "js-agent-1",
              }),
            },
            MODEL_OPTIONS,
          ),
        });
      case "tool-cycle": {
        let modelCalls = 0;
        return Agent.create({
          model: new JsModel(
            {
              request: async () => {
                modelCalls += 1;
                if (modelCalls === 1) {
                  return {
                    text: "",
                    completion_id: "js-tool-1",
                    tool_calls: [{ name: "echo", arguments: { value: "hi" } }],
                  };
                }
                return { text: "tool complete", completion_id: "js-tool-2" };
              },
            },
            MODEL_OPTIONS,
          ),
          toolsets: [
            new JsToolset(
              {
                call: async () => ({ output: { value: "hi" } }),
              },
              TOOL_OPTIONS,
            ),
          ],
          instruction: "Use tools when needed.",
        });
      }
      case "thousand-chunk":
        return Agent.create({
          model: new JsModel(
            {
              request: async () => ({
                async *[Symbol.asyncIterator]() {
                  for (let index = 0; index < 1000; index += 1) {
                    yield { text: "x" };
                  }
                  yield { text: "x".repeat(1000), completion_id: "js-thousand-1" };
                },
              }),
            },
            MODEL_OPTIONS,
          ),
        });
      case "hanging-model":
        return Agent.create({
          model: new JsModel(
            {
              request: (_draft, requestOptions) =>
                new Promise((_resolve, reject) => {
                  requestOptions?.signal?.addEventListener(
                    "abort",
                    () => {
                      reject(requestOptions.signal?.reason ?? new Error("aborted"));
                    },
                    { once: true },
                  );
                }),
            },
            MODEL_OPTIONS,
          ),
        });
      case "hanging-fetch":
        return Agent.create({
          model: new JsModel(
            createOpenAIModel({
              baseUrl: "/finstack/openai?delay=5000",
            }),
            MODEL_OPTIONS,
          ),
        });
      case "persist":
        return Agent.create({
          model: new JsModel(
            {
              request: async () => ({
                text: "hello from persisted Agent",
                completion_id: "js-persist-1",
              }),
            },
            MODEL_OPTIONS,
          ),
          store: persistJournal(options),
        });
      case "hanging-persist":
        return Agent.create({
          model: new JsModel(
            {
              request: (_draft, requestOptions) =>
                new Promise((_resolve, reject) => {
                  requestOptions?.signal?.addEventListener(
                    "abort",
                    () => {
                      reject(requestOptions.signal?.reason ?? new Error("aborted"));
                    },
                    { once: true },
                  );
                }),
            },
            MODEL_OPTIONS,
          ),
          store: persistJournal(options),
        });
      default: {
        const _exhaustive: never = scenario;
        throw new Error(`unsupported worker fixture: ${String(_exhaustive)}`);
      }
    }
  },
});

function readScenario(options: unknown): Scenario {
  if (options === null || typeof options !== "object") {
    return "model-only";
  }
  const scenario = (options as { scenario?: unknown }).scenario;
  switch (scenario) {
    case undefined:
    case "model-only":
    case "tool-cycle":
    case "thousand-chunk":
    case "hanging-model":
    case "hanging-fetch":
    case "persist":
    case "hanging-persist":
      return scenario ?? "model-only";
    default:
      throw new Error(`unsupported worker fixture: ${String(scenario)}`);
  }
}

function persistJournal(options: unknown): JsJournalStore {
  persistDbName = readDbName(options);
  persistStore = new JsJournalStore(createIndexedDbJournalStore({ dbName: persistDbName }), {
    detail: "js_indexeddb_experimental",
  });
  return persistStore;
}

function readDbName(options: unknown): string {
  if (options === null || typeof options !== "object") {
    return "finstack-ai-experimental";
  }
  const dbName = (options as { dbName?: unknown }).dbName;
  return typeof dbName === "string" && dbName.length > 0
    ? dbName
    : "finstack-ai-experimental";
}
