/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";
import { decodeMainToWorker, decodeWorkerToMain } from "./worker-protocol.js";

test("worker protocol rejects non-integral and unbounded counters", () => {
  for (const queueCapacity of [Infinity, Number.NaN, 0, 1.5, 1025]) {
    expect(() =>
      decodeMainToWorker({
        v: 1,
        type: "init",
        id: "init-1",
        queueCapacity,
      }),
    ).toThrow(/invalid queueCapacity/);
  }
  expect(() =>
    decodeMainToWorker({
      v: 1,
      type: "ack",
      agentId: "agent-1",
      runId: "run-1",
      lastSequence: -1.5,
    }),
  ).toThrow(/invalid lastSequence/);
  expect(() =>
    decodeWorkerToMain({
      v: 1,
      type: "batch",
      agentId: "agent-1",
      runId: "run-1",
      firstSequence: Number.MAX_SAFE_INTEGER + 1,
      lastSequence: Number.MAX_SAFE_INTEGER + 1,
      droppedProgress: 0,
      durable: true,
    }),
  ).toThrow(/invalid firstSequence/);
});

test("worker returns correlated errors for invalid initialization bounds", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const code = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    try {
      await window.finstackTest.connectWorker(worker, {
        queueCapacity: Number.POSITIVE_INFINITY,
      });
      return "unexpected_success";
    } catch (error) {
      if (
        error !== null &&
        typeof error === "object" &&
        "code" in error &&
        typeof error.code === "string"
      ) {
        return error.code;
      }
      return "";
    } finally {
      worker.terminate();
    }
  });
  expect(code).toBe("agent_run_invalid_configuration");
});

test("thousand-chunk worker stream does not lock the UI thread", async ({
  page,
}) => {
  await page.goto("/worker-harness.html");
  await page.waitForFunction(() => window.finstackWorkerReady instanceof Promise);
  await page.evaluate(() => window.finstackWorkerReady);

  const result = await page.evaluate(async () => {
    const startTicks = window.finstackWorker.uiTicks();
    const worker = window.finstackWorker.createAgentWorker();
    const client = await window.finstackWorker.connectWorker(worker);
    const agent = await client.create({ scenario: "thousand-chunk" });
    const run = agent.start("stream");
    const batches = [];
    for await (const batch of run.events()) {
      batches.push({
        firstSequence: batch.firstSequence,
        lastSequence: batch.lastSequence,
        kinds: batch.events().map((event) => event.kind),
      });
    }
    const snapshot = await run.result();
    const wasmResources = performance
      .getEntriesByType("resource")
      .filter((entry) => entry.name.endsWith(".wasm"));
    await client.terminate();
    return {
      textLength: snapshot.text.length,
      batchCount: batches.length,
      eventCount: batches.reduce((count, batch) => count + batch.kinds.length, 0),
      terminal: batches.at(-1)?.kinds.at(-1),
      ticksAdvanced: window.finstackWorker.uiTicks() - startTicks,
      wasmOnUi: wasmResources.length,
    };
  });

  expect(result.wasmOnUi).toBe(0);
  expect(result.textLength).toBe(1000);
  expect(result.batchCount).toBeLessThan(result.eventCount);
  expect(result.terminal).toBe("run_completed");
  expect(result.ticksAdvanced).toBeGreaterThan(0);
});

test("worker terminate is cancelled and a new worker can start", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async () => {
    const firstWorker = window.finstackTest.createAgentWorker();
    const first = await window.finstackTest.connectWorker(firstWorker);
    const hanging = await first.create({ scenario: "hanging-model" });
    const run = hanging.start("hang");
    for (let attempt = 0; attempt < 50; attempt += 1) {
      try {
        const _session = run.session;
        break;
      } catch {
        await new Promise((resolve) => {
          setTimeout(resolve, 10);
        });
      }
    }
    let cancelledCode = "";
    let cancelledContext = false;
    const pending = run.result().then(
      () => "completed",
      (error: { code?: string; context?: { runId?: string } }) => {
        cancelledCode = error.code ?? "";
        cancelledContext = typeof error.context?.runId === "string";
        return "rejected";
      },
    );
    await first.terminate();
    const outcome = await pending;

    const secondWorker = window.finstackTest.createAgentWorker();
    const second = await window.finstackTest.connectWorker(secondWorker);
    const agent = await second.create({ scenario: "model-only" });
    const snapshot = await agent.run("hello");
    await second.terminate();
    return { outcome, cancelledCode, cancelledContext, text: snapshot.text };
  });

  expect(result.outcome).toBe("rejected");
  expect(result.cancelledCode).toBe("agent_run_cancelled");
  expect(result.cancelledContext).toBe(true);
  expect(result.text).toBe("hello from Agent");
});

test("cancelled fetch in the worker aborts the hanging proxy", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    const agent = await client.create({ scenario: "hanging-fetch" });
    const run = agent.start("fetch");
    const started = Date.now();
    await new Promise((resolve) => {
      setTimeout(resolve, 50);
    });
    await run.cancel();
    await run.cancel();
    let code = "";
    let context = false;
    try {
      await run.result();
    } catch (error) {
      const record = error as { code?: string; context?: { runId?: string } };
      code = record.code ?? "";
      context = typeof record.context?.runId === "string";
    }
    await client.terminate();
    return { code, context, elapsedMs: Date.now() - started };
  });

  expect(result.code).toBe("agent_run_cancelled");
  expect(result.context).toBe(true);
  expect(result.elapsedMs).toBeLessThan(4000);
});

test("page navigation drops worker result waiters", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
  await page.evaluate(() => {
    const worker = window.finstackTest.createAgentWorker();
    void window.finstackTest.connectWorker(worker).then(async (client) => {
      const agent = await client.create({ scenario: "hanging-model" });
      void agent.start("hang").result();
    });
  });
  await page.goto("/worker-harness.html");
  await page.waitForFunction(() => window.finstackWorkerReady instanceof Promise);
  await page.evaluate(() => window.finstackWorkerReady);
  const text = await page.evaluate(async () => {
    const worker = window.finstackWorker.createAgentWorker();
    const client = await window.finstackWorker.connectWorker(worker);
    const agent = await client.create({ scenario: "model-only" });
    const snapshot = await agent.run("hello");
    await client.terminate();
    return snapshot.text;
  });
  expect(text).toBe("hello from Agent");
});

test("slow UI consumer applies bounded drop-progress backpressure", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker, {
      lagPolicy: "drop-progress",
      queueCapacity: 2,
      durableTimeoutMs: 2000,
    });
    const agent = await client.create({ scenario: "thousand-chunk" });
    const run = agent.start("stream");
    const batches = [];
    for await (const batch of run.events()) {
      batches.push({
        droppedProgress: batch.droppedProgress,
        kinds: batch.events().map((event) => event.kind),
      });
      await new Promise((resolve) => {
        setTimeout(resolve, 20);
      });
    }
    const snapshot = await run.result();
    const pressure = run.backpressure();
    await client.terminate();
    return {
      textLength: snapshot.text.length,
      dropped: batches.reduce((sum, batch) => sum + batch.droppedProgress, 0),
      terminal: batches.at(-1)?.kinds.at(-1),
      pressure,
    };
  });

  expect(result.textLength).toBe(1000);
  expect(result.dropped).toBeGreaterThan(0);
  expect(result.terminal).toBe("run_completed");
  expect(result.pressure.policy).toBe("drop-progress");
  expect(result.pressure.queuedBatches).toBeGreaterThanOrEqual(0);
});

test("main-thread and worker modes pass the same scripted traces", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async () => {
    const collectMain = async (kind: "model-only" | "tool-cycle") => {
      if (kind === "model-only") {
        const model = new window.finstackTest.JsModel(
          {
            request: async () => ({
              text: "hello from Agent",
              completion_id: "js-agent-1",
            }),
          },
          {
            component: "js.model.fixture",
            provider: "js-fixture",
            model: "js-fixture-model",
          },
        );
        const agent = await window.finstackTest.Agent.create({ model });
        const run = agent.start("hello");
        const kinds = [];
        const sequences = [];
        for await (const batch of run.events()) {
          for (const event of batch.events()) {
            kinds.push(event.kind);
            sequences.push(event.transientSequence);
          }
        }
        return { text: (await run.result()).text, kinds, sequences };
      }
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
        {
          component: "js.model.fixture",
          provider: "js-fixture",
          model: "js-fixture-model",
        },
      );
      const toolset = new window.finstackTest.JsToolset(
        {
          call: async () => ({ output: { value: "hi" } }),
        },
        {
          component: "js.toolset.fixture",
          name: "js-fixture-tools",
          tools: [
            {
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
            },
          ],
        },
      );
      const agent = await window.finstackTest.Agent.create({
        model,
        toolsets: [toolset],
        instruction: "Use tools when needed.",
      });
      const run = agent.start("echo hi");
      const kinds = [];
      const sequences = [];
      for await (const batch of run.events()) {
        for (const event of batch.events()) {
          kinds.push(event.kind);
          sequences.push(event.transientSequence);
        }
      }
      return { text: (await run.result()).text, kinds, sequences };
    };

    const collectWorker = async (scenario: "model-only" | "tool-cycle") => {
      const worker = window.finstackTest.createAgentWorker();
      const client = await window.finstackTest.connectWorker(worker);
      const agent = await client.create({ scenario });
      const run = agent.start(scenario === "model-only" ? "hello" : "echo hi");
      const kinds = [];
      const sequences = [];
      for await (const batch of run.events()) {
        for (const event of batch.events()) {
          kinds.push(event.kind);
          sequences.push(event.transientSequence);
        }
      }
      const snapshot = await run.result();
      await client.terminate();
      return { text: snapshot.text, kinds, sequences };
    };

    const mainModel = await collectMain("model-only");
    const workerModel = await collectWorker("model-only");
    const mainTool = await collectMain("tool-cycle");
    const workerTool = await collectWorker("tool-cycle");
    return { mainModel, workerModel, mainTool, workerTool };
  });

  expect(result.workerModel.text).toBe(result.mainModel.text);
  expect(result.workerModel.kinds).toEqual(result.mainModel.kinds);
  expect(result.workerModel.sequences).toEqual(result.mainModel.sequences);
  expect(result.workerTool.text).toBe(result.mainTool.text);
  expect(result.workerTool.kinds).toEqual(result.mainTool.kinds);
  expect(result.workerTool.sequences).toEqual(result.mainTool.sequences);
  expect(result.mainModel.kinds.at(-1)).toBe("run_completed");
  expect(result.mainTool.kinds.at(-1)).toBe("run_completed");
});
