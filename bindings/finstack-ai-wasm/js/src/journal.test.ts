/// <reference path="./harness.d.ts" />

import { readdirSync, readFileSync } from "node:fs";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

import { expect, test } from "@playwright/test";

const repoRoot = join(dirname(fileURLToPath(import.meta.url)), "../../../../");
const journalV1 = join(repoRoot, "fixtures/compatibility/journal/v1");

type JournalFixture = {
  diagnostic_json: unknown;
  payload_digest: string;
  checksum?: string;
  cbor_hex: string;
};

function loadValid(kind: "record-payload" | "envelope"): JournalFixture[] {
  const dir = join(journalV1, kind);
  return readdirSync(dir)
    .filter((name) => name.startsWith("valid--") && name.endsWith(".json"))
    .sort()
    .map((name) => JSON.parse(readFileSync(join(dir, name), "utf8")) as JournalFixture);
}

test("journal known-answers match the Rust corpus through the wasm engine", async ({
  page,
}) => {
  await page.goto("/");
  await page.waitForFunction(() => window.finstackReady instanceof Promise);
  await page.evaluate(() => window.finstackReady);

  const payloads = loadValid("record-payload");
  const envelopes = loadValid("envelope");
  expect(payloads).toHaveLength(40);
  expect(envelopes).toHaveLength(40);

  const observed = await page.evaluate(
    async ({ payloadFixtures, envelopeFixtures }) => {
      const payload = payloadFixtures.map((fixture) =>
        window.finstackTest.journalKnownAnswer("record_body", fixture.diagnostic_json),
      );
      const envelope = envelopeFixtures.map((fixture) =>
        window.finstackTest.journalKnownAnswer(
          "record_envelope",
          fixture.diagnostic_json,
        ),
      );
      return { payload, envelope };
    },
    { payloadFixtures: payloads, envelopeFixtures: envelopes },
  );

  for (const [index, fixture] of payloads.entries()) {
    expect(observed.payload[index]?.payload_digest).toBe(fixture.payload_digest);
    expect(observed.payload[index]?.cbor_hex).toBe(fixture.cbor_hex);
  }
  for (const [index, fixture] of envelopes.entries()) {
    expect(observed.envelope[index]?.payload_digest).toBe(fixture.payload_digest);
    expect(observed.envelope[index]?.checksum).toBe(fixture.checksum);
    expect(observed.envelope[index]?.cbor_hex).toBe(fixture.cbor_hex);
  }
});
