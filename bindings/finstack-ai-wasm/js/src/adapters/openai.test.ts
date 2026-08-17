/// <reference path="../harness.d.ts" />

import { expect, test } from "@playwright/test";

const MODEL_OPTIONS = {
  component: "js.model.fixture",
  provider: "js-fixture",
  model: "js-fixture-model",
};

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
});

test("streams same-origin Responses items without a credential field", async ({
  page,
}) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const defaultUrl = window.finstackTest.openaiDefaultBaseUrl;
    const host = window.finstackTest.createOpenAIModel();
    const raw = await host.request("{}");
    const items: unknown[] = [];
    if (!(raw instanceof ReadableStream)) {
      return { defaultUrl, items, driven: null, kind: typeof raw };
    }
    const reader = raw.getReader();
    while (true) {
      const next = await reader.read();
      if (next.done) {
        break;
      }
      items.push(next.value);
    }
    const driven = await window.finstackTest.driveScriptedModelRequest(
      window.finstackTest.createOpenAIModel(),
      modelOptions,
    );
    return { defaultUrl, items, driven };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.defaultUrl).toBe("/finstack/openai");
  expect(result.items[0]).toEqual({ text: "hel" });
  expect(result.items[1]).toEqual({ text: "lo" });
  expect(result.items.at(-1)).toEqual({
    text: "hello",
    completion_id: "resp-scripted",
  });
  expect(result.driven).toMatchObject({
    ok: true,
    text: "hello",
    completion_id: "resp-scripted",
  });
});

test("propagates AbortSignal through the same-origin fetch battery", async ({
  page,
}) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const host = window.finstackTest.createOpenAIModel({
      baseUrl: "/finstack/openai?delay=250",
    });
    const controller = new AbortController();
    const pending = window.finstackTest.driveScriptedModelRequest(
      host,
      modelOptions,
      controller.signal,
    );
    controller.abort();
    const driven = await pending;
    let directName = "";
    try {
      const direct = window.finstackTest.createOpenAIModel({
        baseUrl: "/finstack/openai?delay=250",
      });
      const again = new AbortController();
      const request = direct.request("{}", { signal: again.signal });
      again.abort();
      await request;
    } catch (error) {
      directName = error instanceof Error ? error.name : String(error);
    }
    return { driven, directName };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.driven).toMatchObject({
    ok: false,
    code: "js_host_cancelled",
    category: "cancellation",
  });
  expect(result.directName).toBe("AbortError");
});
