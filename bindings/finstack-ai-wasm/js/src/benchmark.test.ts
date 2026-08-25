/// <reference path="./harness.d.ts" />

import { expect, test } from "@playwright/test";
import { execSync } from "node:child_process";
import { mkdirSync, readFileSync, writeFileSync } from "node:fs";
import { release } from "node:os";
import { dirname, resolve } from "node:path";
import { fileURLToPath } from "node:url";
import { gzipSync } from "node:zlib";

const HERE = dirname(fileURLToPath(import.meta.url));
const REPO_ROOT = resolve(HERE, "../../../../");
const WASM_PATH = resolve(HERE, "../generated/finstack_ai_wasm_bg.wasm");
const REPORT_PATH = resolve(
  REPO_ROOT,
  "target/performance/wasm-js-crossing.json",
);
const WASM_OVERHEAD_TARGET_PERCENT = 200;
const PAIRED_SAMPLES = 9;
const CROSSING_PAYLOAD_BYTES = 64 * 1024;
const CROSSING_ITERATIONS = 256;

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

  const measured = await page.evaluate(
    async ({ samples, payloadBytes, crossingIterations }) => {
      const now = () => performance.now();
      const median = (values: number[]): number => {
        const sorted = values.slice().sort((left, right) => left - right);
        const value = sorted[Math.floor(sorted.length / 2)];
        if (value === undefined) {
          throw new Error("benchmark sample set was empty");
        }
        return value;
      };
      const crossingPayload = new Uint8Array(payloadBytes);
      const jsCopySamples: number[] = [];
      const wasmRoundTripSamples: number[] = [];
      let crossingChecksum = 0;
      for (let sample = 0; sample < samples; sample += 1) {
        let started = now();
        for (let iteration = 0; iteration < crossingIterations; iteration += 1) {
          crossingPayload[0] = iteration & 0xff;
          const copied = crossingPayload.slice();
          crossingChecksum = (crossingChecksum + (copied[0] ?? 0)) >>> 0;
        }
        jsCopySamples.push((now() - started) / crossingIterations);

        started = now();
        for (let iteration = 0; iteration < crossingIterations; iteration += 1) {
          crossingPayload[0] = iteration & 0xff;
          const copied = window.finstackTest.benchmarkRoundTrip(crossingPayload);
          crossingChecksum = (crossingChecksum + (copied[0] ?? 0)) >>> 0;
        }
        wasmRoundTripSamples.push((now() - started) / crossingIterations);
      }

      const payload = {
        text: "ok",
        completion_id: "bench-1",
      };
      const modelOptions = {
        component: "js.model.bench",
        provider: "js-fixture",
        model: "js-bench-model",
      };
      const hostModel = {
        request: async () => structuredClone(payload),
      };
      const wasmRunSamples: number[] = [];
      let lastText = "";
      for (let index = 0; index < samples; index += 1) {
        const model = new window.finstackTest.JsModel(
          {
            request: async () => hostModel.request(),
          },
          modelOptions,
        );
        const agent = await window.finstackTest.Agent.create({ model });
        const runStarted = now();
        const result = await agent.run("hello");
        wasmRunSamples.push(now() - runStarted);
        lastText = result.text;
      }

      const createStarted = now();
      const warmupModel = new window.finstackTest.JsModel(
        {
          request: async () => hostModel.request(),
        },
        modelOptions,
      );
      const warmupAgent = await window.finstackTest.Agent.create({
        model: warmupModel,
      });
      const createMs = now() - createStarted;
      await warmupAgent.run("hello");

      let events = 0;
      const streamModel = new window.finstackTest.JsModel(
        {
          request: async () => ({
            async *[Symbol.asyncIterator]() {
              for (let index = 0; index < 32; index += 1) {
                yield { text: "x" };
              }
              yield {
                text: "xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx",
                completion_id: "bench-stream",
              };
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
        runMs: median(wasmRunSamples),
        jsCopyMs: median(jsCopySamples),
        wasmRoundTripMs: median(wasmRoundTripSamples),
        streamMs,
        events,
        text: lastText,
        samples,
        crossingChecksum,
      };
    },
    {
      samples: PAIRED_SAMPLES,
      payloadBytes: CROSSING_PAYLOAD_BYTES,
      crossingIterations: CROSSING_ITERATIONS,
    },
  );

  expect(measured.text).toBe("ok");
  expect(measured.crossingChecksum).toBeGreaterThan(0);
  expect(measured.jsCopyMs).toBeGreaterThan(0);
  expect(measured.wasmRoundTripMs).toBeGreaterThan(0);
  const overhead = (measured.wasmRoundTripMs / measured.jsCopyMs - 1) * 100;
  expect(overhead).toBeLessThanOrEqual(WASM_OVERHEAD_TARGET_PERCENT);

  const ns = (ms: number) => Math.round(ms * 1_000_000);
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
      samples: measured.samples,
      comparison: "batched_typed_array_copy_vs_wasm_round_trip",
      crossing_payload_bytes: CROSSING_PAYLOAD_BYTES,
      crossing_iterations: CROSSING_ITERATIONS,
    },
    timing_ns: {
      init_median: ns(initMs),
      create_median: ns(measured.createMs),
      reducer_run_median: ns(measured.runMs),
      event_throughput_median: ns(measured.streamMs),
      host_callback_median: ns(measured.jsCopyMs),
      wasm_drive_median: ns(measured.wasmRoundTripMs),
    },
    binding: {
      overhead_percent: overhead,
      target_percent: WASM_OVERHEAD_TARGET_PERCENT,
      within_target: overhead <= WASM_OVERHEAD_TARGET_PERCENT,
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
  mkdirSync(dirname(REPORT_PATH), { recursive: true });
  writeFileSync(REPORT_PATH, `${JSON.stringify(report, null, 2)}\n`);
});
