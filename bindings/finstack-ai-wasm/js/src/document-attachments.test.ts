/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";

const MODEL_OPTIONS = {
  component: "js.model.attachments",
  provider: "js-fixture",
  model: "js-fixture-model",
};

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
});

test("run with one csv attachment completes and the model sees parsed Markdown with no File block", async ({
  page,
}) => {
  const result = await page.evaluate(async ({ modelOptions }) => {
    const parseDraft = (draft: unknown): unknown =>
      typeof draft === "string" ? JSON.parse(draft) : draft;
    const requests: unknown[] = [];
    const model = new window.finstackTest.JsModel(
      {
        request: async (draft) => {
          requests.push(parseDraft(draft));
          return { text: "acknowledged", completion_id: "js-attachment-1" };
        },
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const encoder = new TextEncoder();
    const runResult = await agent.run("Summarize the attached file.", {
      attachments: [
        {
          data: encoder.encode("quarter,revenue\nQ1,1250\n"),
          mediaType: "text/csv",
          name: "revenue.csv",
        },
      ],
    });

    const request = requests[0] as { messages: Array<{ role: string; content: Array<{ kind: string; text?: string }> }> };
    const userBlocks = request.messages
      .filter((message) => message.role === "user")
      .flatMap((message) => message.content);
    const hasFileBlock = userBlocks.some((block) => block.kind === "file");
    const modelVisibleText = userBlocks
      .filter((block) => block.kind === "text")
      .map((block) => block.text ?? "")
      .join("");

    return {
      text: runResult.text,
      requestCount: requests.length,
      hasFileBlock,
      modelVisibleText,
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.text).toBe("acknowledged");
  expect(result.requestCount).toBe(1);
  expect(result.hasFileBlock).toBe(false);
  expect(result.modelVisibleText).toContain("Q1");
  expect(result.modelVisibleText.toLowerCase()).toContain("revenue");
});

test("more than 8 attachments are rejected with an attachment-count error", async ({
  page,
}) => {
  const message = await page.evaluate(async ({ modelOptions }) => {
    const model = new window.finstackTest.JsModel(
      {
        request: async () => ({ text: "ok", completion_id: "js-attachment-2" }),
      },
      modelOptions,
    );
    const agent = await window.finstackTest.Agent.create({ model });
    const encoder = new TextEncoder();
    const attachment = {
      data: encoder.encode("a,b\n1,2\n"),
      mediaType: "text/csv",
    };
    try {
      await agent.run("x", { attachments: Array(9).fill(attachment) });
    } catch (error) {
      return error instanceof Error ? error.message : String(error);
    }
    return undefined;
  }, { modelOptions: MODEL_OPTIONS });

  expect(message).toBeDefined();
  expect(message?.toLowerCase()).toContain("attachment");
});
