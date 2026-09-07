/// <reference path="./harness.d.ts" />

import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";
import { expect, test } from "@playwright/test";

const MODEL_OPTIONS = {
  component: "js.model.fixture",
  provider: "js-fixture",
  model: "js-fixture-model",
};

const EXAMPLE_DIR = join(
  dirname(fileURLToPath(import.meta.url)),
  "../../../../examples/browser-minimal",
);

test("map-backed host journal can append, load, and inspect", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async ({ modelOptions }) => {
    const host = window.finstackTest.createMapJournalStore();
    const health = await host.health();
    const store = new window.finstackTest.JsJournalStore(host, {
      detail: "js_indexeddb_experimental",
    });
    const agent = await window.finstackTest.Agent.create({
      model: new window.finstackTest.JsModel(
        {
          request: async () => ({
            text: "hello from map store",
            completion_id: "js-map-1",
          }),
        },
        modelOptions,
      ),
      store,
    });
    const runResult = await agent.run("hello");
    const snapshot = await window.finstackTest.inspectSession(
      store,
      runResult.session.sessionId,
    );
    return {
      healthDetail: health.detail,
      text: runResult.text,
      phase: snapshot.phase,
      resultText: snapshot.resultText,
      headSequence: snapshot.headSequence,
    };
  }, { modelOptions: MODEL_OPTIONS });

  expect(result.healthDetail).toBe("js_map_experimental");
  expect(result.text).toBe("hello from map store");
  expect(result.phase).toBe("completed");
  expect(result.resultText).toBe("hello from map store");
  expect(result.headSequence).toBeGreaterThan(0);
});

test("browser minimal page runs and inspects the same session after reload", async ({ page }) => {
  const errors: string[] = [];
  page.on("pageerror", (error) => errors.push(error.message));
  await page.goto("/examples/browser-minimal/");
  await page.getByRole("button", { name: "Run scripted session" }).click();
  await expect(page.locator("#result")).toHaveText("hello from browser-minimal");
  await expect(page.locator("#events")).toContainText("run_completed");
  await page.getByRole("button", { name: "Inspect last session" }).click();
  await expect(page.locator("#inspect-panel")).toContainText('"phase": "completed"');
  const inspected: unknown = JSON.parse(await page.locator("#inspect-panel").innerText());
  expect(inspected).toMatchObject({ resultText: "hello from browser-minimal", phase: "completed" });

  await page.reload();
  await page.getByRole("button", { name: "Inspect last session" }).click();
  await expect(page.locator("#inspect-panel")).toContainText('"phase": "completed"');
  expect(JSON.parse(await page.locator("#inspect-panel").innerText())).toEqual(inspected);
  expect(errors).toEqual([]);
});

test("completed worker session survives reload as inspect", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const first = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    const agent = await client.create({
      scenario: "persist",
      dbName: "pr037-a01",
    });
    const snapshot = await agent.run("hello");
    await client.terminate();
    return {
      sessionId: snapshot.sessionId,
      text: snapshot.text,
    };
  });

  expect(first.text).toBe("hello from persisted Agent");

  await page.reload();
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const inspected = await page.evaluate(async ({ sessionId }) => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    await client.create({ scenario: "persist", dbName: "pr037-a01" });
    const snapshot = await client.inspectSession(sessionId);
    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open("pr037-a01", 3);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("open failed"));
    });
    const batchCount = await new Promise<number>((resolve, reject) => {
      const request = db.transaction("batches", "readonly").objectStore("batches").count();
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("count failed"));
    });
    db.close();
    await client.terminate();
    return {
      phase: snapshot.phase,
      resultText: snapshot.resultText,
      sessionId: snapshot.sessionId,
      batchCount,
    };
  }, { sessionId: first.sessionId });

  expect(inspected.phase).toBe("completed");
  expect(inspected.resultText).toBe(first.text);
  expect(inspected.sessionId).toBe(first.sessionId);
  expect(inspected.batchCount).toBeGreaterThan(0);
});

test("interrupted persist inspect is provisional and memory cannot resume", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const first = await page.evaluate(async () => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    const agent = await client.create({
      scenario: "hanging-persist",
      dbName: "pr037-a02",
    });
    const run = agent.start("hang");
    for (let attempt = 0; attempt < 50; attempt += 1) {
      try {
        const session = run.session;
        for (let persistAttempt = 0; persistAttempt < 50; persistAttempt += 1) {
          const snapshot = await client.inspectSession(session.sessionId);
          if (snapshot.headSequence > 0) {
            await client.terminate();
            return { sessionId: session.sessionId };
          }
          await new Promise((resolve) => {
            setTimeout(resolve, 10);
          });
        }
        await client.terminate();
        throw new Error("hanging persist did not commit a journal head");
      } catch (error) {
        if (error instanceof Error && error.message.includes("journal head")) {
          throw error;
        }
        await new Promise((resolve) => {
          setTimeout(resolve, 10);
        });
      }
    }
    await client.terminate();
    throw new Error("hanging persist did not accept");
  });

  await page.reload();
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const inspected = await page.evaluate(async ({ sessionId }) => {
    const worker = window.finstackTest.createAgentWorker();
    const client = await window.finstackTest.connectWorker(worker);
    await client.create({ scenario: "hanging-persist", dbName: "pr037-a02" });
    const snapshot = await client.inspectSession(sessionId);
    await client.terminate();
    const memory = window.finstackTest.createAgentWorker();
    const memoryClient = await window.finstackTest.connectWorker(memory);
    const hanging = await memoryClient.create({ scenario: "hanging-model" });
    const run = hanging.start("hang");
    for (let attempt = 0; attempt < 50; attempt += 1) {
      try {
        const session = run.session;
        await memoryClient.terminate();
        return {
          phase: snapshot.phase,
          headSequence: snapshot.headSequence,
          memorySessionId: session.sessionId,
        };
      } catch {
        await new Promise((resolve) => {
          setTimeout(resolve, 10);
        });
      }
    }
    await memoryClient.terminate();
    throw new Error("memory worker did not accept");
  }, { sessionId: first.sessionId });

  expect(["in_progress", "cancelled"]).toContain(inspected.phase);
  expect(inspected.headSequence).toBeGreaterThan(0);
  expect(inspected.memorySessionId).not.toBe(first.sessionId);
});

test("indexeddb journal enforces CAS, integrity, order, and artifact ceilings", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const result = await page.evaluate(async () => {
    const dbName = "pr037-a03";
    const errorCode = (error: unknown): string => {
      if (
        error !== null &&
        typeof error === "object" &&
        "code" in error &&
        typeof error.code === "string"
      ) {
        return error.code;
      }
      return "";
    };
    await window.finstackTest.deleteIndexedDbStores({ dbName });
    const store = window.finstackTest.createIndexedDbJournalStore({ dbName });
    const health = await store.health();
    const sessionId = "01234567-89ab-7cde-89ab-0123456789a1";
    const first = {
      batch_id: "01234567-89ab-7cde-89ab-0123456789b1",
      session_id: sessionId,
      expected_sequence: 1,
      records: [
        {
          format_version: 1,
          kind_version: 1,
          record_id: "01234567-89ab-7cde-89ab-0123456789c1",
          session_id: sessionId,
          lane_id: "01234567-89ab-7cde-89ab-0123456789d1",
          timestamp: "1970-01-01T00:00:00.000Z",
          derived_event_ids: [],
          body: { run_accepted: { cycle: 0 } },
        },
      ],
    };
    const firstReceipt = (await store.append?.(JSON.stringify(first))) as {
      batch_id: string;
      first_sequence: number;
    };
    const retry = (await store.append?.(JSON.stringify(first))) as {
      batch_id: string;
      first_sequence: number;
    };
    let conflictCode = "";
    try {
      await store.append?.(
        JSON.stringify({
          ...first,
          batch_id: "01234567-89ab-7cde-89ab-0123456789b2",
          expected_sequence: 99,
          records: [
            {
              format_version: 1,
              kind_version: 1,
              record_id: "01234567-89ab-7cde-89ab-0123456789c2",
              session_id: sessionId,
              lane_id: "01234567-89ab-7cde-89ab-0123456789d1",
              timestamp: "1970-01-01T00:00:00.000Z",
              derived_event_ids: [],
              body: { run_accepted: { cycle: 0 } },
            },
          ],
        }),
      );
    } catch (error) {
      conflictCode = errorCode(error);
    }
    const second = {
      batch_id: "01234567-89ab-7cde-89ab-0123456789b3",
      session_id: sessionId,
      expected_sequence: 2,
      records: [
        {
          format_version: 1,
          kind_version: 1,
          record_id: "01234567-89ab-7cde-89ab-0123456789c3",
          session_id: sessionId,
          lane_id: "01234567-89ab-7cde-89ab-0123456789d1",
          timestamp: "1970-01-01T00:00:00.001Z",
          derived_event_ids: [],
          body: { run_completed: { cycle: 0 } },
        },
      ],
    };
    await store.append?.(JSON.stringify(second));
    const loaded = (await store.load?.(JSON.stringify({ session_id: sessionId }))) as {
      head_sequence: number;
      committed_batches: Array<{ first_sequence: number }>;
    };

    const db = await new Promise<IDBDatabase>((resolve, reject) => {
      const request = indexedDB.open(dbName, 3);
      request.onsuccess = () => resolve(request.result);
      request.onerror = () => reject(request.error ?? new Error("open failed"));
    });
    await new Promise<void>((resolve, reject) => {
      const request = db
        .transaction("batches", "readwrite")
        .objectStore("batches")
        .put({
          sessionId,
          firstSequence: 99,
          batchId: "01234567-89ab-7cde-89ab-0123456789ff",
          requestDigest: "{}",
          committedJson: "{not-json",
          recordIds: [],
        });
      request.onsuccess = () => resolve();
      request.onerror = () => reject(request.error ?? new Error("put failed"));
    });
    db.close();
    let corruptCode = "";
    try {
      await store.load?.(JSON.stringify({ session_id: sessionId }));
    } catch (error) {
      corruptCode = errorCode(error);
    }

    await new Promise<void>((resolve, reject) => {
      const request = indexedDB.open(dbName, 3);
      request.onsuccess = () => {
        const opened = request.result;
        const write = opened
          .transaction("meta", "readwrite")
          .objectStore("meta")
          .put({ key: "schemaVersion", value: 4 });
        write.onsuccess = () => {
          opened.close();
          resolve();
        };
        write.onerror = () => reject(write.error ?? new Error("meta put failed"));
      };
      request.onerror = () => reject(request.error ?? new Error("reopen failed"));
    });
    let schemaCode = "";
    try {
      await window.finstackTest.createIndexedDbJournalStore({ dbName }).health();
    } catch (error) {
      schemaCode = errorCode(error);
    }

    await window.finstackTest.deleteIndexedDbStores({ dbName: "pr037-a03-blobs" });
    const artifacts = window.finstackTest.createIndexedDbArtifactStore({
      dbName: "pr037-a03-blobs",
      maxBlobBytes: 16,
    });
    const blob = JSON.stringify({ id: "blob-1" });
    const artifact = JSON.stringify({ blob: JSON.parse(blob) });
    await artifacts.stagePut("{}", new Uint8Array([1, 2, 3]), "{}", artifact, "blob-1");
    const firstGet = Array.from(await artifacts.get("{}", artifact, "blob-1"));
    const [, byBlobBytes] = await artifacts.getByBlob("{}", blob);
    const artifactDbOpen = Promise.withResolvers<IDBDatabase>();
    const artifactDbRequest = indexedDB.open("pr037-a03-blobs", 3);
    artifactDbRequest.onsuccess = () => artifactDbOpen.resolve(artifactDbRequest.result);
    artifactDbRequest.onerror = () =>
      artifactDbOpen.reject(artifactDbRequest.error ?? new Error("open failed"));
    const artifactDb = await artifactDbOpen.promise;

    const storedRead = Promise.withResolvers<{
      bytes: Uint8Array;
      blobKey: string;
    }>();
    const storedRequest = artifactDb
      .transaction("artifacts", "readonly")
      .objectStore("artifacts")
      .get("blob-1");
    storedRequest.onsuccess = () =>
      storedRead.resolve(storedRequest.result as { bytes: Uint8Array; blobKey: string });
    storedRequest.onerror = () =>
      storedRead.reject(storedRequest.error ?? new Error("artifact read failed"));
    const stored = await storedRead.promise;

    const statsRead = Promise.withResolvers<{ count: number; totalBytes: number }>();
    const statsRequest = artifactDb
      .transaction("meta", "readonly")
      .objectStore("meta")
      .get("artifactStats");
    statsRequest.onsuccess = () =>
      statsRead.resolve(statsRequest.result as { count: number; totalBytes: number });
    statsRequest.onerror = () =>
      statsRead.reject(statsRequest.error ?? new Error("stats read failed"));
    const stats = await statsRead.promise;
    artifactDb.close();
    let oversizeCode = "";
    try {
      await artifacts.stagePut("{}", new Uint8Array(32), "{}", "ref-big", "blob-big");
    } catch (error) {
      oversizeCode = errorCode(error);
    }
    let invalidLimitCode = "";
    try {
      window.finstackTest.createIndexedDbArtifactStore({
        dbName: "invalid-limit",
        maxBlobBytes: Number.POSITIVE_INFINITY,
      });
    } catch (error) {
      invalidLimitCode = errorCode(error);
    }
    let unsafeSequenceCode = "";
    try {
      await store.append?.(
        JSON.stringify({
          ...first,
          batch_id: "01234567-89ab-7cde-89ab-0123456789ee",
          expected_sequence: Number.MAX_SAFE_INTEGER + 1,
        }),
      );
    } catch (error) {
      unsafeSequenceCode = errorCode(error);
    }
    return {
      healthDetail: health.detail,
      firstBatch: firstReceipt.batch_id,
      retryBatch: retry.batch_id,
      retrySequence: retry.first_sequence,
      conflictCode,
      sequences: loaded.committed_batches.map((batch) => batch.first_sequence),
      headSequence: loaded.head_sequence,
      corruptCode,
      schemaCode,
      firstGet,
      byBlob: Array.from(byBlobBytes),
      oversizeCode,
      typedArtifactBytes: stored.bytes instanceof Uint8Array,
      indexedBlobKey: stored.blobKey,
      artifactCount: stats.count,
      artifactBytes: stats.totalBytes,
      invalidLimitCode,
      unsafeSequenceCode,
    };
  });

  expect(result.healthDetail).toBe("js_indexeddb_experimental");
  expect(result.retryBatch).toBe(result.firstBatch);
  expect(result.retrySequence).toBe(1);
  expect(result.conflictCode).toBe("store_conflict");
  expect(result.sequences).toEqual([1, 2]);
  expect(result.headSequence).toBe(2);
  expect(result.corruptCode).toBe("store_integrity_failure");
  expect(result.schemaCode).toBe("store_integrity_failure");
  expect(result.firstGet).toEqual([1, 2, 3]);
  expect(result.byBlob).toEqual([1, 2, 3]);
  expect(result.oversizeCode).toBe("store_limit_exceeded");
  expect(result.typedArtifactBytes).toBe(true);
  expect(result.indexedBlobKey).toBe(JSON.stringify({ id: "blob-1" }));
  expect(result.artifactCount).toBe(1);
  expect(result.artifactBytes).toBe(3);
  expect(result.invalidLimitCode).toBe("agent_run_invalid_configuration");
  expect(result.unsafeSequenceCode).toBe("store_integrity_failure");

  await page.reload();
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
  const reloaded = await page.evaluate(async () => {
    const artifacts = window.finstackTest.createIndexedDbArtifactStore({
      dbName: "pr037-a03-blobs",
      maxBlobBytes: 16,
    });
    const blob = JSON.stringify({ id: "blob-1" });
    const artifact = JSON.stringify({ blob: JSON.parse(blob) });
    return Array.from(await artifacts.get("{}", artifact, "blob-1"));
  });
  expect(reloaded).toEqual([1, 2, 3]);
});

test("browser-minimal uses public package imports only", () => {
  const main = readFileSync(join(EXAMPLE_DIR, "main.ts"), "utf8");
  const worker = readFileSync(join(EXAMPLE_DIR, "worker.ts"), "utf8");
  const sources = `${main}\n${worker}`;
  expect(sources).toContain('from "@finstack/ai"');
  expect(sources).toContain('from "@finstack/ai/worker"');
  expect(sources).toContain('from "@finstack/ai/adapters/indexeddb"');
  expect(sources).not.toMatch(/from ["']\.\.\/generated/);
  expect(sources).not.toMatch(/finstack-ai-wasm\/src/);
});

test("package and example docs label persistence experimental", async ({ page }) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);
  const detail = await page.evaluate(async () => {
    const health = await window.finstackTest.createIndexedDbJournalStore({
      dbName: "pr037-a05",
    }).health();
    return health.detail;
  });
  expect(detail).toBe("js_indexeddb_experimental");

  const packageReadme = readFileSync(
    join(dirname(fileURLToPath(import.meta.url)), "../README.md"),
    "utf8",
  );
  const exampleReadme = readFileSync(join(EXAMPLE_DIR, "README.md"), "utf8");
  expect(packageReadme).toContain("Persistence remains experimental");
  expect(exampleReadme).toContain("Persistence remains experimental");
  expect(exampleReadme).toContain("not crash-durable");
});
