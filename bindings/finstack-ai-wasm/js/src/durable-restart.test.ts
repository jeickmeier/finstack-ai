/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";

test("IndexedDB reload stays experimental and inspects the same session", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const first = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    const agent = await client.create({
      scenario: "persist",
      dbName: "pr048-a10",
    });
    const snapshot = await agent.run("hello");
    const health = await window.finstackTest
      .createIndexedDbJournalStore({ dbName: "pr048-a10" })
      .health();
    await client.terminate();
    return {
      sessionId: snapshot.sessionId,
      text: snapshot.text,
      ready: health.ready,
      detail: health.detail,
    };
  });

  expect(first.ready).toBe(true);
  expect(first.detail).toBe("js_indexeddb_experimental");

  await page.reload();
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const inspected = await page.evaluate(async ({ sessionId }) => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    await client.create({ scenario: "persist", dbName: "pr048-a10" });
    const snapshot = await client.inspectSession(sessionId);
    const health = await window.finstackTest
      .createIndexedDbJournalStore({ dbName: "pr048-a10" })
      .health();
    await client.terminate();
    return {
      phase: snapshot.phase,
      resultText: snapshot.resultText,
      sessionId: snapshot.sessionId,
      ready: health.ready,
      detail: health.detail,
    };
  }, { sessionId: first.sessionId });

  expect(inspected.phase).toBe("completed");
  expect(inspected.resultText).toBe(first.text);
  expect(inspected.sessionId).toBe(first.sessionId);
  expect(inspected.ready).toBe(true);
  expect(inspected.detail).toBe("js_indexeddb_experimental");
});
