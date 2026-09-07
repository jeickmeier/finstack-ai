/// <reference path="./harness.d.ts" />

/**
 * Golden-questions conformance for the browser knowledge example
 * (`examples/browser-knowledge`), plus determinism and schema checks for
 * its TypeScript retrieval toolset.
 *
 * Same fixture and same two assertions as the Rust tests
 * (`apps/finstack-knowledge/src/golden.rs`) and the Python pytest
 * (`examples/python-notebooks/test_knowledge_golden.py`): the answer
 * contains every `must_contain` needle, and the observed event kinds are
 * a superset of `event_kinds_expected`.
 */

import { readFileSync } from "node:fs";
import { createServer } from "node:http";
import type { AddressInfo } from "node:net";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

// Single source: the same module the example worker and page import.
import {
  CORPUS,
  RETRIEVAL_TOOL,
  rankCorpus,
} from "../../../../examples/browser-knowledge/retrieval.mjs";

const MODEL_OPTIONS = {
  component: "know.model.golden",
  provider: "js-knowledge",
  model: "js-knowledge-model",
};

interface GoldenEntry {
  id: string;
  question: string;
  scripted_response: string;
  must_contain: string[];
  event_kinds_expected: string[];
}

const FIXTURE_PATH = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../apps/finstack-knowledge/fixtures/golden.json",
);
const GOLDEN: GoldenEntry[] = (
  JSON.parse(readFileSync(FIXTURE_PATH, "utf-8")) as { entries: GoldenEntry[] }
).entries;

test("retrieval ranking is deterministic with stable tie-breaks", () => {
  const first = rankCorpus(CORPUS, "runtime ports journal", 3);
  const second = rankCorpus(CORPUS, "runtime ports journal", 3);
  expect(first).toEqual(second);
  expect(first.length).toBeGreaterThan(0);
  expect(first[0]?.doc_id).toBe("architecture");
  // No-match queries return nothing rather than noise.
  expect(rankCorpus(CORPUS, "zebra xylophone", 2)).toEqual([]);
});

test("retrieval tool schema is a valid cached tool spec", () => {
  const spec = RETRIEVAL_TOOL as Record<string, unknown>;
  for (const key of [
    "id",
    "model_name",
    "title",
    "description",
    "input_schema",
    "output_schema",
    "execution",
    "side_effect",
    "retry_safety",
    "approval",
    "max_result_bytes",
    "metadata",
  ]) {
    expect(spec[key], key).toBeDefined();
  }
  expect(spec.model_name).toBe("search_corpus");
  expect(spec.side_effect).toBe("read_only");
});

test("a scripted search_corpus call weaves the excerpt into the answer", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(
    async ({ modelOptions }) => {
      const retrieval = (await import(
        "/examples/browser-knowledge/retrieval.mjs"
      )) as {
        CORPUS: { id: string }[];
        RETRIEVAL_TOOLSET_OPTIONS: { component: string; name: string; tools: unknown[] };
        createRetrievalHost: (corpus: unknown) => {
          call: (context: unknown, call: unknown) => Promise<unknown>;
        };
      };
      let modelCalls = 0;
      let excerptSeen = "";
      const model = new window.finstackTest.JsModel(
        {
          request: async (request: unknown) => {
            modelCalls += 1;
            if (modelCalls === 1) {
              return {
                text: "",
                completion_id: "know-golden-1",
                tool_calls: [
                  {
                    name: "search_corpus",
                    arguments: { query: "runtime ports journal", top_k: 1 },
                  },
                ],
              };
            }
            // Read the tool result back out of the follow-up request (the
            // draft arrives as a JSON string): the excerpt must have flowed
            // through the run, not this script.
            const draft =
              typeof request === "string" ? JSON.parse(request) : request;
            const messages = (draft as { messages: unknown[] }).messages ?? [];
            for (const message of messages) {
              for (const block of (message as { content?: unknown[] }).content ?? []) {
                const nested = (block as { content?: { value?: unknown }[] }).content;
                const value = nested?.[0]?.value as
                  | { excerpts?: { excerpt: string; doc_id: string }[] }
                  | undefined;
                const top = value?.excerpts?.[0];
                if (top !== undefined) {
                  excerptSeen = `${top.doc_id}: ${top.excerpt}`;
                }
              }
            }
            return {
              text: `From the docs — ${excerptSeen}`,
              completion_id: "know-golden-2",
            };
          },
        },
        modelOptions,
      );
      const toolset = new window.finstackTest.JsToolset(
        retrieval.createRetrievalHost(retrieval.CORPUS),
        retrieval.RETRIEVAL_TOOLSET_OPTIONS,
      );
      const agent = await window.finstackTest.Agent.create({
        model,
        toolsets: [toolset],
        instruction: "Search the corpus and cite doc ids.",
      });
      const runResult = await agent.run("What are the six runtime ports?");
      return { text: runResult.text, modelCalls };
    },
    { modelOptions: MODEL_OPTIONS },
  );

  expect(result.modelCalls).toBe(2);
  expect(result.text).toContain("architecture:");
  expect(result.text).toContain("six");
});

test("browser knowledge page asks, reloads, and passes every golden question", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/examples/browser-knowledge/");
  await page.getByRole("button", { name: "Ask", exact: true }).click();
  await expect(page.locator("#answer")).toContainText("From the architecture doc:");
  await expect(page.locator("#answer")).toContainText("six");
  await expect(page.locator("#events")).toContainText("run_completed");
  const answer = await page.locator("#answer").innerText();
  await page.getByRole("button", { name: "Inspect last session" }).click();
  await expect(page.locator("#inspect-panel")).toContainText('"phase": "completed"');
  const inspected: unknown = JSON.parse(await page.locator("#inspect-panel").innerText());
  expect(inspected).toMatchObject({ resultText: answer, phase: "completed" });

  await page.reload();
  await page.getByRole("button", { name: "Inspect last session" }).click();
  await expect(page.locator("#inspect-panel")).toContainText('"phase": "completed"');
  expect(JSON.parse(await page.locator("#inspect-panel").innerText())).toEqual(inspected);

  await page.getByRole("button", { name: "Run golden set" }).click();
  await expect(page.locator("#golden-results")).toHaveText(
    GOLDEN.map((entry) => `PASS  ${entry.id}`).join("\n"),
  );
  expect(errors).toEqual([]);
});

test("browser knowledge worker connects before retrieval finishes loading", async ({ page, baseURL }) => {
  let release!: () => void;
  let requested = false;
  let connected = false;
  const blocked = new Promise<void>((resolve) => { release = resolve; });
  const server = createServer(async (request, response) => {
    try {
      if (request.url === "/examples/browser-knowledge/retrieval.mjs") {
        requested = true;
        await blocked;
      }
      const upstream = await fetch(new URL(request.url ?? "/", baseURL));
      response.writeHead(upstream.status, {
        "content-type": upstream.headers.get("content-type") ?? "application/octet-stream",
      });
      response.end(Buffer.from(await upstream.arrayBuffer()));
    } catch (error) {
      response.destroy(error instanceof Error ? error : undefined);
    }
  });
  await new Promise<void>((resolve) => server.listen(0, "127.0.0.1", resolve));
  const origin = `http://127.0.0.1:${(server.address() as AddressInfo).port}`;
  try {
    await page.goto(origin);
    await page.waitForFunction(() => window.finstackReady instanceof Promise);
    await page.evaluate(() => window.finstackReady);
    await page.exposeFunction("knowledgeWorkerConnected", () => { connected = true; });
    const connection = page.evaluate(async () => {
      const worker = new Worker("/examples/browser-knowledge/dist/worker.js", { type: "module" });
      try {
        const client = await window.finstackTest.connectWorker(worker);
        await (window as unknown as {
          knowledgeWorkerConnected(): Promise<void>;
        }).knowledgeWorkerConnected();
        const agent = await client.create({});
        return (await agent.run("What are the six runtime ports?")).text;
      } finally {
        worker.terminate();
      }
    });
    const outcome = connection.then(
      (text) => ({ text }),
      (error: unknown) => ({ error }),
    );
    await expect.poll(() => requested).toBe(true);
    await expect.poll(() => connected).toBe(true);
    release();
    const result = await outcome;
    if ("error" in result) {
      throw result.error;
    }
    expect(result.text).toContain("From the architecture doc:");
  } finally {
    release();
    await page.close();
    server.closeAllConnections();
    await new Promise<void>((resolve) => server.close(() => resolve()));
  }
});

test("golden entries hold in the browser", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  for (const entry of GOLDEN) {
    const observed = await page.evaluate(
      async ({ modelOptions, scripted, question, id }) => {
        const retrieval = (await import(
          "/examples/browser-knowledge/retrieval.mjs"
        )) as {
          CORPUS: unknown;
          RETRIEVAL_TOOLSET_OPTIONS: { component: string; name: string; tools: unknown[] };
          createRetrievalHost: (corpus: unknown) => {
            call: (context: unknown, call: unknown) => Promise<unknown>;
          };
        };
        const model = new window.finstackTest.JsModel(
          {
            request: async () => ({
              text: scripted,
              completion_id: `golden-${id}`,
            }),
          },
          modelOptions,
        );
        const agent = await window.finstackTest.Agent.create({
          model,
          toolsets: [
            new window.finstackTest.JsToolset(
              retrieval.createRetrievalHost(retrieval.CORPUS),
              retrieval.RETRIEVAL_TOOLSET_OPTIONS,
            ),
          ],
          instruction: "Answer from the corpus; cite sources.",
        });
        const run = agent.start(question);
        const kinds: string[] = [];
        for await (const batch of run.events()) {
          for (const event of batch.events()) {
            kinds.push(event.kind);
          }
        }
        const snapshot = await run.result();
        return { text: snapshot.text, kinds };
      },
      {
        modelOptions: MODEL_OPTIONS,
        scripted: entry.scripted_response,
        question: entry.question,
        id: entry.id,
      },
    );

    for (const needle of entry.must_contain) {
      expect(observed.text, `${entry.id}: must contain ${needle}`).toContain(needle);
    }
    for (const kind of entry.event_kinds_expected) {
      expect(observed.kinds, `${entry.id}: expected kind ${kind}`).toContain(kind);
    }
  }
});
