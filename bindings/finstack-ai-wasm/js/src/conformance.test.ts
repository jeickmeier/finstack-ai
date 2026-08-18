/// <reference path="./harness.d.ts" />

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

const TS_ALPHA_INSTALL = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../examples/ts-alpha-install",
);

const MODEL_OPTIONS = {
  component: "js.model.capability",
  provider: "js-fixture",
  model: "js-capability-model",
};

function recordsInOrder(trace: string[], expected: string[]): boolean {
  let index = 0;
  for (const kind of trace) {
    if (index < expected.length && kind === expected[index]) {
      index += 1;
    }
  }
  return index === expected.length;
}

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

test.describe("browser goldens", () => {
  test.beforeEach(async ({ page }) => {
    await page.goto("/");
    await page.waitForFunction(() => window.finstackReady instanceof Promise);
    await page.evaluate(() => window.finstackReady);
  });

test("all activation modes share Rust-owned traces and a stable prefix", async ({
  page,
}) => {
  const golden = await page.evaluate(async () => {
    const response = await fetch(
      "/fixtures/golden-trace/v1/structured-output/valid--pr012-capabilities.json",
    );
    return (await response.json()) as { records: string[] };
  });

  const result = await page.evaluate(async ({ modelOptions }) => {
    const requests: unknown[] = [];
    const textValues = (value: unknown): string[] => {
      if (Array.isArray(value)) {
        return value.flatMap((nested) => textValues(nested));
      }
      if (value !== null && typeof value === "object") {
        return Object.entries(value as Record<string, unknown>).flatMap(
          ([key, nested]) => {
            const items = textValues(nested);
            return key === "text" || typeof nested !== "string" ? items : [];
          },
        );
      }
      return typeof value === "string" ? [value] : [];
    };
    const parseDraft = (draft: unknown): unknown =>
      typeof draft === "string" ? JSON.parse(draft) : draft;

    const model = new window.finstackTest.JsModel(
      {
        request: async (draft) => {
          requests.push(parseDraft(draft));
          return {
            text: "ok",
            completion_id: `capability-${requests.length}`,
          };
        },
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({
      model,
      instruction: "Stable prefix.",
      capabilities: [
        {
          id: "js.capability.always",
          description: "Baseline safety guidance",
          instructions: ["Always instruction."],
          activation: "always",
        },
        {
          id: "js.capability.application",
          description: "Application selected accounting guidance",
          instructions: ["Application instruction."],
          activation: "application",
        },
        {
          id: "js.capability.research",
          description: "Research financial statements",
          instructions: ["Research instruction."],
          activation: "model",
        },
      ],
      activeCapabilities: ["js.capability.application"],
    });
    const first = await agent.run("say hello");
    const second = await agent.run("research financial statements", {
      capability: "js.capability.research",
    });
    return {
      catalog: agent.capabilityCatalog(),
      compact: agent.compactCapabilityCatalog(),
      firstTrace: first.trace,
      secondTrace: second.trace,
      firstActive: first.activeCapabilities,
      secondActive: second.activeCapabilities,
      firstText: textValues(
        (requests[0] as { messages?: unknown } | undefined)?.messages,
      ),
      secondText: textValues(
        (requests[1] as { messages?: unknown } | undefined)?.messages,
      ),
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.catalog).toEqual([
    {
      id: "js.capability.research",
      description: "Research financial statements",
    },
  ]);
  expect(result.compact).toBe(
    "js.capability.research: Research financial statements",
  );
  expect(result.firstActive).toEqual([
    { id: "js.capability.always", source: "always" },
    { id: "js.capability.application", source: "application" },
  ]);
  expect(result.secondActive).toEqual([
    { id: "js.capability.always", source: "always" },
    { id: "js.capability.application", source: "application" },
    { id: "js.capability.research", source: "model" },
  ]);
  expect(recordsInOrder(result.firstTrace, golden.records)).toBe(true);
  expect(recordsInOrder(result.secondTrace, golden.records)).toBe(true);
  expect(result.firstText.indexOf("Stable prefix.")).toBeLessThan(
    result.firstText.indexOf("Always instruction."),
  );
  expect(result.secondText.indexOf("Stable prefix.")).toBeLessThan(
    result.secondText.indexOf("Always instruction."),
  );
  expect(result.firstText.slice(0, 2)).toEqual(result.secondText.slice(0, 2));
  expect(result.firstText).not.toContain("Research instruction.");
  expect(result.secondText).toContain("Research instruction.");
});

test("capability configuration fails closed for model activation", async ({
  page,
}) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "unused",
          completion_id: "unused",
        }),
      },
      modelOptions,
    );
    try {
      await window.finstackTest.Agent.create({
        model,
        capabilities: [
          {
            id: "js.capability.model-only",
            description: "Model selected capability",
            instructions: ["Model instruction."],
            activation: "model",
          },
        ],
        activeCapabilities: ["js.capability.model-only"],
      });
      return { ok: true, code: "", message: "" };
    } catch (error) {
      const typed = error as { code?: string; message?: string };
      return {
        ok: false,
        code: typed.code ?? "",
        message: typed.message ?? String(error),
      };
    }
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.ok).toBe(false);
  expect(result.code).toBe("agent_run_invalid_configuration");
  expect(result.message).toContain("capability_not_application_activated");
});

test("tool-batch records match the shared continue-model golden slice", async ({
  page,
}) => {
  const golden = await page.evaluate(async () => {
    const response = await fetch(
      "/fixtures/golden-trace/v1/tool-batch/valid--pr010-continue-model.json",
    );
    return (await response.json()) as { records_after_open: string[] };
  });

  const result = await page.evaluate(
    async ({ modelOptions, tool }) => {
      let modelCalls = 0;
      const model = new window.finstackTest.JsModel(
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
        modelOptions,
      );
      const toolset = new window.finstackTest.JsToolset(
        {
          call: async () => ({ output: { value: "hi" } }),
        },
        {
          component: "js.toolset.fixture",
          name: "js-fixture-tools",
          tools: [tool],
        },
      );
      const agent = await window.finstackTest.Agent.create({
        model,
        toolsets: [toolset],
        instruction: "Use tools when needed.",
      });
      const runResult = await agent.run("echo hi");
      return { text: runResult.text, trace: runResult.trace, modelCalls };
    },
    { modelOptions: MODEL_OPTIONS, tool: ECHO_TOOL },
  );

  expect(result.text).toBe("tool complete");
  expect(result.modelCalls).toBe(2);
  const start = result.trace.indexOf("tool_batch_opened");
  expect(start).toBeGreaterThanOrEqual(0);
  const expected = golden.records_after_open;
  const semantic = result.trace
    .slice(start)
    .filter((kind) => expected.includes(kind));
  expect(semantic.slice(0, expected.length)).toEqual(expected);
});
});

test("ts-alpha-install uses public package imports only", () => {
  const main = readFileSync(join(TS_ALPHA_INSTALL, "main.ts"), "utf8");
  expect(main).toContain('from "@finstack/ai"');
  expect(main).not.toMatch(/from ["']\.\.\/generated/);
  expect(main).not.toMatch(/finstack-ai-wasm\/src/);
  expect(main).not.toMatch(/js\/src/);
});
