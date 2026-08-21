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

const CHILD_RUN_PREPARED = {
  parent_run_id: "01234567-89ab-7cde-89ab-0123456789ab",
  parent_effect_id: "01234567-89ab-7cde-89ab-0123456789ad",
  child: {
    operation: {
      tenant_scope: "tenant-a",
      session_id: "01234567-89ab-7cde-89ab-0123456789ae",
      lane_id: "01234567-89ab-7cde-89ab-0123456789af",
      run_id: "01234567-89ab-7cde-89ab-0123456789b0",
    },
  },
  request_digest: "00".repeat(32),
  placement: "compatible_lane_in_parent_session",
};

const INTERACTION_RESOLUTION = {
  locator: {
    tenant_scope: "tenant-a",
    session_id: "01234567-89ab-7cde-89ab-012345679201",
    lane_id: "01234567-89ab-7cde-89ab-012345679202",
    run_id: "01234567-89ab-7cde-89ab-012345679203",
  },
  resolution: {
    interaction_id: "01234567-89ab-7cde-89ab-012345679205",
    resolution_id: "resolution-1",
    principal: {
      issuer: "https://issuer.example",
      subject: "user-1",
      tenant_scope: "tenant-a",
    },
    authorization: {
      policy_version: "policy-v1",
      decision_id: "decision-1",
    },
    response: {
      approved: true,
    },
    comment: "approved",
  },
};

const EXTERNAL_EFFECT_COMPLETION = {
  locator: {
    tenant_scope: "tenant-a",
    session_id: "01234567-89ab-7cde-89ab-012345679201",
    lane_id: "01234567-89ab-7cde-89ab-012345679202",
    run_id: "01234567-89ab-7cde-89ab-012345679203",
  },
  principal: {
    issuer: "https://issuer.example",
    subject: "user-1",
    tenant_scope: "tenant-a",
  },
  authorization: {
    policy_version: "policy-v1",
    decision_id: "decision-1",
  },
  completion: {
    effect_id: "01234567-89ab-7cde-89ab-012345679204",
    completion_id: "completion-1",
    outcome: {
      failed: {
        error: {
          code: "provider_failed",
          message: "provider failed",
          category: "model",
          retryable: true,
          identifiers: {},
          safe_details: {},
        },
      },
    },
  },
};

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
});

test("constructs host wrappers and drives scripted model/tool DTOs", async ({
  page,
}) => {
  const result = await page.evaluate(
    async ({ modelOptions, toolOptions }) => {
      const modelHost = {
        request: async () => ({
          text: "hello from JS",
          completion_id: "js-1",
        }),
      };
      const toolHost = {
        call: async () => ({
          output: { value: "hi" },
          is_error: false,
        }),
      };
      new window.finstackTest.JsModel(modelHost, modelOptions);
      new window.finstackTest.JsToolset(toolHost, toolOptions);
      new window.finstackTest.JsContextProvider(
        {
          collect: async () => ({
            items: [],
            estimated_tokens: 0,
            bytes: 0,
            cache_key: null,
          }),
        },
        { component: "js.context.fixture" },
      );
      new window.finstackTest.JsMiddleware(
        { invoke: async () => "continue" },
        { component: "js.middleware.fixture", stages: ["before_run"] },
      );
      new window.finstackTest.JsObserver(
        { observe: async () => undefined },
        { component: "js.observer.fixture" },
      );
      new window.finstackTest.JsClock(window.finstackTest.createHostClock());
      new window.finstackTest.JsRandomSource(
        window.finstackTest.createHostRandomSource(),
      );
      const artifactStore = window.finstackTest.createMemoryArtifactStore({
          storeId: "host-test",
          maxArtifactBytes: 4 * 1024 * 1024,
          maxArtifacts: 64,
          maxTotalBytes: 16 * 1024 * 1024,
          maxOwnersPerArtifact: 16,
          orphanGraceMs: 60_000,
          maxGcBatch: 32,
        });
      new window.finstackTest.JsArtifactStore(artifactStore);
      const blob = JSON.stringify({ id: "blob-1" });
      const artifact = JSON.stringify({ blob: JSON.parse(blob) });
      await artifactStore.stagePut(
        "{}",
        new Uint8Array([1, 2, 3]),
        "{}",
        artifact,
        "storage-1",
      );
      const [, artifactBytes] = await artifactStore.getByBlob("{}", blob);
      return {
        model: await window.finstackTest.driveScriptedModelRequest(
          modelHost,
          modelOptions,
        ),
        tool: await window.finstackTest.driveScriptedToolCall(
          toolHost,
          toolOptions,
        ),
        artifactBytes: Array.from(artifactBytes),
      };
    },
    { modelOptions: MODEL_OPTIONS, toolOptions: TOOL_OPTIONS },
  );

  expect(result.model).toEqual({
    ok: true,
    text: "hello from JS",
    completion_id: "js-1",
    tool_calls: [],
  });
  expect(result.tool).toEqual({
    ok: true,
    output: { value: "hi" },
    is_error: false,
  });
  expect(result.artifactBytes).toEqual([1, 2, 3]);
});

test("maps AbortSignal cancellation before settle and mid-stream", async ({
  page,
}) => {
  const result = await page.evaluate(
    async ({ modelOptions }) => {
      const hanging = {
        request: (_draft: unknown, options?: { signal?: AbortSignal }) =>
          new Promise((_resolve, reject) => {
            const signal = options?.signal;
            if (signal?.aborted) {
              reject(new DOMException("Aborted", "AbortError"));
              return;
            }
            signal?.addEventListener("abort", () => {
              reject(new DOMException("Aborted", "AbortError"));
            });
          }),
      };
      const before = new AbortController();
      const pendingBefore = window.finstackTest.driveScriptedModelRequest(
        hanging,
        modelOptions,
        before.signal,
      );
      before.abort();
      const cancelledBefore = await pendingBefore;

      let started!: () => void;
      const sawFirst = new Promise<void>((resolve) => {
        started = resolve;
      });
      const streaming = {
        request: (_draft: unknown, options?: { signal?: AbortSignal }) =>
          new ReadableStream({
            start(controller) {
              controller.enqueue({ text: "hel" });
              started();
              options?.signal?.addEventListener("abort", () => {
                controller.error(new DOMException("Aborted", "AbortError"));
              });
            },
          }),
      };
      const mid = new AbortController();
      const pendingMid = window.finstackTest.driveScriptedModelRequest(
        streaming,
        modelOptions,
        mid.signal,
      );
      await sawFirst;
      mid.abort();
      const cancelledMid = await pendingMid;
      return { cancelledBefore, cancelledMid };
    },
    { modelOptions: MODEL_OPTIONS },
  );

  expect(result.cancelledBefore).toMatchObject({
    ok: false,
    code: "js_host_cancelled",
    category: "cancellation",
  });
  expect(result.cancelledMid).toMatchObject({
    ok: false,
    code: "js_host_cancelled",
    category: "cancellation",
  });
  expect(JSON.stringify(result)).not.toContain("TypeError");
});

test("rejects malformed host objects with a stable invalid code", async ({
  page,
}) => {
  const result = await page.evaluate(
    async ({ modelOptions, toolOptions }) => {
      const extra = await window.finstackTest.driveScriptedModelRequest(
        {
          request: async () => ({
            text: "hello",
            completion_id: "js-1",
            unexpected: true,
          }),
        },
        modelOptions,
      );
      const missing = await window.finstackTest.driveScriptedModelRequest(
        {
          request: async () => ({ text: "hello" }),
        },
        modelOptions,
      );
      const scalar = await window.finstackTest.driveScriptedToolCall(
        {
          call: async () => ({ output: "nope" }),
        },
        toolOptions,
      );
      return { extra, missing, scalar };
    },
    { modelOptions: MODEL_OPTIONS, toolOptions: TOOL_OPTIONS },
  );

  expect(result.extra).toMatchObject({
    ok: false,
    code: "js_host_result_invalid",
    category: "validation",
  });
  expect(result.missing).toMatchObject({
    ok: false,
    code: "js_host_result_invalid",
    category: "validation",
  });
  expect(result.scalar).toMatchObject({
    ok: false,
    code: "js_host_result_invalid",
    category: "validation",
  });
  expect(JSON.stringify(result)).not.toContain("unexpected");
});

test("accepts a completed object, ReadableStream, or async iterable", async ({
  page,
}) => {
  const result = await page.evaluate(
    async ({ modelOptions }) => {
      const completed = await window.finstackTest.driveScriptedModelRequest(
        {
          request: async () => ({
            text: "one shot",
            completion_id: "js-object-1",
          }),
        },
        modelOptions,
      );
      const streamed = await window.finstackTest.driveScriptedModelRequest(
        {
          request: async () =>
            new ReadableStream({
              start(controller) {
                controller.enqueue({ text: "hel" });
                controller.enqueue({
                  text: "lo",
                  completion_id: "js-stream-1",
                });
                controller.close();
              },
            }),
        },
        modelOptions,
      );
      const iterated = await window.finstackTest.driveScriptedModelRequest(
        {
          request: async () => ({
            async *[Symbol.asyncIterator]() {
              yield { text: "hel" };
              yield { text: "lo", completion_id: "js-iter-1" };
            },
          }),
        },
        modelOptions,
      );
      return { completed, streamed, iterated };
    },
    { modelOptions: MODEL_OPTIONS },
  );

  expect(result.completed).toMatchObject({
    ok: true,
    text: "one shot",
    completion_id: "js-object-1",
  });
  expect(result.streamed).toMatchObject({
    ok: true,
    text: "lo",
    completion_id: "js-stream-1",
  });
  expect(result.iterated).toMatchObject({
    ok: true,
    text: "lo",
    completion_id: "js-iter-1",
  });
});

test("normalizes pre-beta shapes and applies scripted coordinator commands", async ({
  page,
}) => {
  const result = await page.evaluate(
    ({ child, interaction, external }) => {
      const normalizedChild = window.finstackTest.normalizePrebetaShape(
        "child_run_prepared",
        child,
      );
      const normalizedInteraction = window.finstackTest.normalizePrebetaShape(
        "interaction_resolution",
        interaction,
      );
      const normalizedExternal = window.finstackTest.normalizePrebetaShape(
        "external_effect_completion",
        external,
      );
      const applied = window.finstackTest.applyScriptedCoordinatorCommands([
        { kind: "child_run_prepared", value: child },
        { kind: "interaction_resolution", value: interaction },
        { kind: "external_effect_completion", value: external },
      ]);
      let invalidReason = "";
      try {
        window.finstackTest.normalizePrebetaShape("child_run_prepared", {
          ...child,
          unexpected: true,
        });
      } catch (error) {
        invalidReason = error instanceof Error ? error.message : String(error);
      }
      let unsupportedReason = "";
      try {
        window.finstackTest.normalizePrebetaShape(
          "not_a_kind" as "child_run_prepared",
          child,
        );
      } catch (error) {
        unsupportedReason =
          error instanceof Error ? error.message : String(error);
      }
      return {
        normalizedChild,
        normalizedInteraction,
        normalizedExternal,
        applied,
        invalidReason,
        unsupportedReason,
      };
    },
    {
      child: CHILD_RUN_PREPARED,
      interaction: INTERACTION_RESOLUTION,
      external: EXTERNAL_EFFECT_COMPLETION,
    },
  );

  expect(result.normalizedChild).toEqual(CHILD_RUN_PREPARED);
  expect(result.normalizedInteraction).toEqual(INTERACTION_RESOLUTION);
  expect(result.normalizedExternal).toEqual(EXTERNAL_EFFECT_COMPLETION);
  expect(result.applied.commands).toEqual([
    {
      kind: "child_run_prepared",
      status: "applied",
      parent_run_id: CHILD_RUN_PREPARED.parent_run_id,
      parent_effect_id: CHILD_RUN_PREPARED.parent_effect_id,
      child_run_id: CHILD_RUN_PREPARED.child.operation.run_id,
    },
    {
      kind: "interaction_resolution",
      status: "applied",
      run_id: INTERACTION_RESOLUTION.locator.run_id,
      session_id: INTERACTION_RESOLUTION.locator.session_id,
    },
    {
      kind: "external_effect_completion",
      status: "applied",
      run_id: EXTERNAL_EFFECT_COMPLETION.locator.run_id,
      session_id: EXTERNAL_EFFECT_COMPLETION.locator.session_id,
    },
  ]);
  expect(result.invalidReason).toContain("invalid pre-beta shape");
  expect(result.unsupportedReason).toContain("unsupported pre-beta shape");
});

test("scripted journal store is ready and never durable", async ({ page }) => {
  const result = await page.evaluate(async () => {
    const adapter = window.finstackTest.createMemoryJournalStore();
    new window.finstackTest.JsJournalStore(adapter);
    return window.finstackTest.driveScriptedJournalHealth(adapter);
  });

  expect(result).toMatchObject({
    ok: true,
    ready: true,
    durable: false,
    detail: "js_memory_prebeta",
  });
});
