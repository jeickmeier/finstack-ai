/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";

test("loads health, metadata, port fixtures, and the no-op trace", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const health = await page.evaluate(() => window.finstackTest.health());
  expect(health).toBe("ok");

  const metadata = await page.evaluate(() => window.finstackTest.buildMetadata());
  expect(metadata.version).toBe("0.0.4");
  expect(metadata.engineVersion).toBe("0.0.4");
  expect(metadata.implementation).toBe("wasm");
  expect(metadata.target).toBe("wasm32-unknown-unknown");

  await page.evaluate(() => {
    window.finstackTest.compilePortProxies();
  });

  const report = await page.evaluate(() =>
    JSON.parse(window.finstackTest.runNoopTrace()) as {
      durable_records: unknown[];
      normalized_events: unknown[];
      matched: boolean;
    },
  );
  expect(report.matched).toBe(true);
  expect(report.durable_records).toEqual([]);
  expect(report.normalized_events).toEqual([]);
});
