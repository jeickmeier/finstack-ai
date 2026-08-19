/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";

test.beforeEach(async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
});

test("parseDocumentMarkdown returns the parsed Markdown for a csv document", async ({
  page,
}) => {
  const markdown = await page.evaluate(() => {
    const encoder = new TextEncoder();
    const data = encoder.encode("quarter,revenue\nQ1,1250\nQ2,1310\n");
    return window.finstackTest.parseDocumentMarkdown(data, "text/csv");
  });

  expect(markdown).toContain("1250");
  expect(markdown.toLowerCase()).toContain("revenue");
});

test("parseDocument returns the full detailed result for a csv document", async ({ page }) => {
  const result = await page.evaluate(() => {
    const encoder = new TextEncoder();
    const data = encoder.encode("quarter,revenue\nQ1,1250\nQ2,1310\n");
    return window.finstackTest.parseDocument(data, "text/csv");
  });

  expect(result.format).toBe("csv");
  expect(result.requires_ocr).toBe(false);
  expect(result.truncated).toBe(false);
  expect(result.page_count).toBeNull();
  expect(result.classification).toBeNull();
  expect(result.markdown).toContain("1250");
});

test("parseDocumentMarkdown throws a stable document_* error for unsupported input", async ({
  page,
}) => {
  const message = await page.evaluate(() => {
    const encoder = new TextEncoder();
    const data = encoder.encode("not a document");
    try {
      window.finstackTest.parseDocumentMarkdown(data, "application/octet-stream");
    } catch (error) {
      return error instanceof Error ? error.message : String(error);
    }
    return undefined;
  });

  expect(message).toBeDefined();
  expect(message).toContain("document_unsupported_format");
});
