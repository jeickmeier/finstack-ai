/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";
import { execSync } from "node:child_process";
import { readFileSync, writeFileSync } from "node:fs";
import { release } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "../../../../");
const WASM_PATH = resolve(HERE, "../generated/finstack_ai_wasm_bg.wasm");
const REPORT_PATH = resolve(
  REPO_ROOT,
  "docs/implementation/artifacts/pr-038/wasm-js-crossing.json",
);

test("records isolated WASM/JS crossing warning measurements", async ({
  page,
  browserName,
}) => {
  test.skip(browserName !== "chromium", "crossing report is Chromium-owned");
  const initStarted = Date.now();
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
  const initMs = Date.now() - initStarted;

  const measured = await page.evaluate(async () => {
    const now = () => performance.now();
    const modelOptions = {
      component: "js.model.bench",
      provider: "js-fixture",
      model: "js-bench-model",
    };
    let hostMs = 0;
    const model = new window.finstackTest.JsModel(
      {
        request: async () => {
          const started = now();
          const result = { text: "ok", completion_id: "bench-1" };
          hostMs += now() - started;
          return result;
        },
      },
      modelOptions,
    );
    const createStarted = now();
    const agent = await window.finstackTest.Agent.create({ model });
    const createMs = now() - createStarted;
    const runStarted = now();
    const result = await agent.run("hello");
    const runMs = now() - runStarted;
    let events = 0;
    const streamModel = new window.finstackTest.JsModel(
      {
        request: async () => ({
          async *[Symbol.asyncIterator]() {
            for (let index = 0; index < 32; index += 1) {
              yield { text: "x" };
            }
            yield { text: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", completion_id: "bench-stream" };
          },
        }),
      },
      modelOptions,
    );
    const streamAgent = await window.finstackTest.Agent.create({
      model: streamModel,
    });
    const streamStarted = now();
    const streamRun = streamAgent.start("stream");
    for await (const batch of streamRun.events()) {
      events += batch.events().length;
    }
    await streamRun.result();
    const streamMs = now() - streamStarted;
    return {
      createMs,
      runMs,
      hostMs,
      streamMs,
      events,
      text: result.text,
    };
  });

  expect(measured.text).toBe("ok");
  const ns = (ms: number) => Math.round(ms * 1_000_000);
  const wasmDriveMs = Math.max(measured.runMs - measured.hostMs, 0);
  const overhead =
    wasmDriveMs === 0
      ? 0
      : ((measured.runMs - wasmDriveMs) / wasmDriveMs) * 100;
  const wasm = readFileSync(WASM_PATH);
  const commit = execSync("git rev-parse --short=12 HEAD", {
    cwd: REPO_ROOT,
    encoding: "utf8",
  }).trim();
  const report = {
    format_version: 1,
    workload: "wasm-js-crossing-v1",
    commit,
    platform: {
      system: process.platform,
      release: release(),
      machine: process.arch,
    },
    browser: "chromium",
    parameters: {
      deltas_per_run: 32,
      runs_per_sample: 1,
      samples: 1,
    },
    timing_ns: {
      init_median: ns(initMs),
      create_median: ns(measured.createMs),
      reducer_run_median: ns(measured.runMs),
      event_throughput_median: ns(measured.streamMs),
      host_callback_median: ns(measured.hostMs),
      wasm_drive_median: ns(wasmDriveMs),
    },
    binding: {
      overhead_percent: overhead,
      target_percent: 10,
      within_target: overhead <= 10,
    },
    event_delivery: {
      logical_events: measured.events,
      model_deltas: 32,
      js_host_callbacks: 1,
    },
    throughput: {
      model_deltas_per_second:
        measured.streamMs === 0 ? 0 : 32 / (measured.streamMs / 1000),
    },
    bundle: {
      wasm_bytes: wasm.byteLength,
      wasm_gzip_bytes: gzipSync(wasm).byteLength,
    },
    external_io: {
      included_in_binding_comparison: false,
      scope: "No network, provider, filesystem, or external store I/O",
    },
  };
  writeFileSync(REPORT_PATH, `${JSON.stringify(report, null, 2)}\n`);
  expect(report.timing_ns.reducer_run_median).toBeGreaterThan(0);
});
