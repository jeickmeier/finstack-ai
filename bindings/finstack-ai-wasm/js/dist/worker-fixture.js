import { Agent, JsModel, JsToolset, init } from "./index.js";
import { createOpenAICompatibleModel } from "./adapters/openai-compatible.js";
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
const wasmReady = init();
exposeWorkerHost({
    async create(options) {
        await wasmReady;
        const scenario = readScenario(options);
        switch (scenario) {
            case "model-only":
                return Agent.create({
                    model: new JsModel({
                        request: async () => ({
                            text: "hello from Agent",
                            completion_id: "js-agent-1",
                        }),
                    }, MODEL_OPTIONS),
                });
            case "tool-cycle": {
                let modelCalls = 0;
                return Agent.create({
                    model: new JsModel({
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
                    }, MODEL_OPTIONS),
                    toolsets: [
                        new JsToolset({
                            call: async () => ({ output: { value: "hi" } }),
                        }, TOOL_OPTIONS),
                    ],
                    instruction: "Use tools when needed.",
                });
            }
            case "thousand-chunk":
                return Agent.create({
                    model: new JsModel({
                        request: async () => ({
                            async *[Symbol.asyncIterator]() {
                                for (let index = 0; index < 1000; index += 1) {
                                    yield { text: "x" };
                                }
                                yield { text: "x".repeat(1000), completion_id: "js-thousand-1" };
                            },
                        }),
                    }, MODEL_OPTIONS),
                });
            case "hanging-model":
                return Agent.create({
                    model: new JsModel({
                        request: (_draft, requestOptions) => new Promise((_resolve, reject) => {
                            requestOptions?.signal?.addEventListener("abort", () => {
                                reject(requestOptions.signal?.reason ?? new Error("aborted"));
                            }, { once: true });
                        }),
                    }, MODEL_OPTIONS),
                });
            case "hanging-fetch":
                return Agent.create({
                    model: new JsModel(createOpenAICompatibleModel({
                        baseUrl: "/finstack/openai?delay=5000",
                    }), MODEL_OPTIONS),
                });
            default: {
                const _exhaustive = scenario;
                throw new Error(`unsupported worker fixture: ${String(_exhaustive)}`);
            }
        }
    },
});
function readScenario(options) {
    if (options === null || typeof options !== "object") {
        return "model-only";
    }
    const scenario = options.scenario;
    switch (scenario) {
        case undefined:
        case "model-only":
        case "tool-cycle":
        case "thousand-chunk":
        case "hanging-model":
        case "hanging-fetch":
            return scenario ?? "model-only";
        default:
            throw new Error(`unsupported worker fixture: ${String(scenario)}`);
    }
}
//# sourceMappingURL=worker-fixture.js.map