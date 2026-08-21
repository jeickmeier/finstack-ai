/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";

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

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
});

test("runs a model-only scripted Agent to a text result", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "hello from Agent",
          completion_id: "js-agent-1",
        }),
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const runResult = await agent.run("hello");
    return {
      text: runResult.text,
      snapshot: runResult.toDict(),
      session: runResult.session.toDict(),
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.text).toBe("hello from Agent");
  expect(result.snapshot.text).toBe("hello from Agent");
  expect(result.snapshot.tenantScope).toBe("js-local");
  expect(result.session).toEqual({
    tenantScope: result.snapshot.tenantScope,
    sessionId: result.snapshot.sessionId,
    laneId: result.snapshot.laneId,
    runId: result.snapshot.runId,
  });
});

test("reports bounded redacted observer diagnostics without affecting results", async ({
  page,
}) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "observer failures stay non-semantic",
          completion_id: "js-observer-1",
        }),
      },
      modelOptions,
    );
    const observer = new window.finstackTest.JsObserver(
      {
        observe: async () => {
          throw new Error("host secret must not escape");
        },
      },
      { component: "js.observer.failing", payloadMode: "redacted" },
    );
    const agent = await window.finstackTest.Agent.create({
      model,
      observers: [observer],
    });
    const run = agent.start("hello");
    const runResult = await run.result();
    const diagnostics = await run.observerDiagnostics();
    return {
      text: runResult.text,
      total: diagnostics.total.toString(),
      dropped: diagnostics.dropped.toString(),
      recent: diagnostics.recent,
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.text).toBe("observer failures stay non-semantic");
  expect(BigInt(result.total)).toBeGreaterThanOrEqual(1n);
  expect(result.dropped).toBe("0");
  expect(result.recent.length).toBeGreaterThan(0);
  expect(result.recent[0]).toEqual({
    code: "observer_delivery_failed",
    detail: "observer delivery failed",
  });
  expect(JSON.stringify(result.recent)).not.toContain("secret");
});

test("completes a scripted tool-using Agent cycle", async ({ page }) => {
  const result = await page.evaluate(
    async ({ modelOptions, toolOptions }) => {
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
        toolOptions,
      );
      const agent = await window.finstackTest.Agent.create({
        model,
        toolsets: [toolset],
        instruction: "Use tools when needed.",
      });
      const runResult = await agent.run("echo hi");
      return { text: runResult.text, modelCalls };
    },
    { modelOptions: MODEL_OPTIONS, toolOptions: TOOL_OPTIONS },
  );

  expect(result.text).toBe("tool complete");
  expect(result.modelCalls).toBe(2);
});

test("keeps durable state in WASM until explicit snapshots", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          async *[Symbol.asyncIterator]() {
            yield { text: "hel" };
            yield { text: "lo" };
            yield { text: "hello", completion_id: "js-snap-1" };
          },
        }),
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const run = agent.start("snapshot");
    const batches = [];
    for await (const batch of run.events()) {
      batches.push({
        firstSequence: batch.firstSequence,
        lastSequence: batch.lastSequence,
        json: batch.toJson(),
        jsonAgain: batch.toJson(),
        bytes: Array.from(batch.toJsonBytes()),
        events: batch.events().map((event) => ({
          kind: event.kind,
          eventClass: event.eventClass,
          transientSequence: event.transientSequence,
          json: event.toJson(),
          jsonAgain: event.toJson(),
        })),
      });
    }
    const runResult = await run.result();
    return {
      session: run.locator.toDict(),
      sessionAgain: run.locator.toDict(),
      result: runResult.toDict(),
      batches,
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.session).toEqual(result.sessionAgain);
  expect(result.result.sessionId).toBe(result.session.sessionId);
  expect(result.batches.length).toBeGreaterThan(0);
  for (const batch of result.batches) {
    expect(batch.json).toBe(batch.jsonAgain);
    expect(new TextDecoder().decode(Uint8Array.from(batch.bytes))).toBe(batch.json);
    expect(batch.firstSequence).toBeLessThanOrEqual(batch.lastSequence);
    for (const event of batch.events) {
      expect(event.json).toBe(event.jsonAgain);
      expect(JSON.parse(event.json).kind).toBe(event.kind);
    }
  }
});

test("batches multi-delta events and preserves terminal order", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const parts = ["h", "e", "l", "l", "o", " ", "w", "o", "r", "l", "d"];
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          async *[Symbol.asyncIterator]() {
            for (const part of parts) {
              yield { text: part };
            }
            yield { text: parts.join(""), completion_id: "js-multi-1" };
          },
        }),
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const run = agent.start("say hello");
    const batches = [];
    for await (const batch of run.events()) {
      batches.push({
        firstSequence: batch.firstSequence,
        lastSequence: batch.lastSequence,
        kinds: batch.events().map((event) => event.kind),
        sequences: batch.events().map((event) => event.transientSequence),
      });
    }
    const runResult = await run.result();
    const kinds = batches.flatMap((batch) => batch.kinds);
    const sequences = batches.flatMap((batch) => batch.sequences);
    return {
      text: runResult.text,
      batchCount: batches.length,
      eventCount: kinds.length,
      kinds,
      sequences,
      firstLast: batches.map((batch) => [batch.firstSequence, batch.lastSequence]),
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.text).toBe("hello world");
  expect(result.batchCount).toBeLessThan(result.eventCount);
  expect(result.sequences).toEqual([...result.sequences].sort((left, right) => left - right));
  expect(result.kinds).toContain("model_text_delta");
  expect(result.kinds.at(-1)).toBe("run_completed");
  expect(result.firstLast.every(([first, last]) => first <= last)).toBe(true);
});

test("closeEvents and iterator drop leave result completable", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "still completes",
          completion_id: "js-close-1",
        }),
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const closed = agent.start("close events");
    await closed.closeEvents();
    const closedText = (await closed.result()).text;

    const dropped = agent.start("drop iterator");
    const iterator = dropped.events()[Symbol.asyncIterator]();
    await iterator.return?.();
    const droppedText = (await dropped.result()).text;

    return { closedText, droppedText };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.closedText).toBe("still completes");
  expect(result.droppedText).toBe("still completes");
});

test("double cancel is idempotent and drop does not cancel", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    let pending = 0;
    const hanging = new window.finstackTest.JsModel(
      {
        request: (_draft, options) => {
          pending += 1;
          return new Promise((_resolve, reject) => {
            const signal = options?.signal;
            if (signal?.aborted) {
              pending -= 1;
              reject(new DOMException("Aborted", "AbortError"));
              return;
            }
            signal?.addEventListener(
              "abort",
              () => {
                pending -= 1;
                reject(new DOMException("Aborted", "AbortError"));
              },
              { once: true },
            );
          });
        },
      },
      modelOptions,
    );
    const completing = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "not cancelled",
          completion_id: "js-drop-1",
        }),
      },
      modelOptions,
    );
    const cancelAgent = await window.finstackTest.Agent.create({ model: hanging });
    const run = cancelAgent.start("wait");
    const session = run.locator.toDict();
    const started = Date.now();
    while (pending === 0 && Date.now() - started < 2000) {
      await new Promise((resolve) => setTimeout(resolve, 0));
    }
    await Promise.all([run.cancel(), run.cancel()]);
    let cancelled:
      | {
          code: string;
          retryable: boolean;
          context?: unknown;
          name: string;
        }
      | undefined;
    try {
      await run.result();
    } catch (error) {
      const wrapped = window.finstackTest.FinstackError.fromUnknown(error);
      cancelled = {
        name: wrapped.name,
        code: wrapped.code,
        retryable: wrapped.retryable,
        context: wrapped.context,
      };
    }

    const dropAgent = await window.finstackTest.Agent.create({ model: completing });
    const kept = dropAgent.start("keep");
    const dropped = dropAgent.start("drop");
    void dropped;
    const keptText = (await kept.result()).text;

    return { session, cancelled, pending, keptText };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.cancelled).toEqual({
    name: "FinstackError",
    code: "agent_run_cancelled",
    retryable: false,
    context: result.session,
  });
  expect(result.pending).toBe(0);
  expect(result.keptText).toBe("not cancelled");
});

test("accepts per_call and informed_batch approval grants", async ({ page }) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({
          text: "granted",
          completion_id: "js-approval-grant-1",
        }),
      },
      modelOptions,
    );
    const perCall = await window.finstackTest.Agent.create({
      model,
      approvalGrant: window.finstackTest.ApprovalGrantMode.perCall(),
    });
    const informed = await window.finstackTest.Agent.create({
      model,
      approvalGrant: window.finstackTest.ApprovalGrantMode.informedBatch(),
    });
    const omitted = await window.finstackTest.Agent.create({ model });
    const texts = [
      (await perCall.run("hello")).text,
      (await informed.run("hello")).text,
      (await omitted.run("hello")).text,
    ];
    return { texts };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.texts).toEqual(["granted", "granted", "granted"]);
});

test("rejects an unknown approval grant mode", async ({ page }) => {
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
        approvalGrant: "not_a_mode" as "per_call",
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
  expect(result.message).toContain("approval_grant must be per_call or informed_batch");
});
