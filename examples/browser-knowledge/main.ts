/**
 * Page controller for the browser knowledge example: question box, live
 * event-kind stream, answer with citations, session inspection, and the
 * golden-questions conformance table.
 */

import { connectWorker } from "@finstack/ai/worker";
import { deleteIndexedDbStores } from "@finstack/ai/adapters/indexeddb";

const answerEl = document.getElementById("answer");
const eventsEl = document.getElementById("events");
const inspectEl = document.getElementById("inspect-panel");
const goldenEl = document.getElementById("golden-results");
const questionEl = document.getElementById("question") as HTMLInputElement | null;

const LAST_SESSION_KEY = "finstack-know-browser-last-session";

const worker = new Worker(new URL("./worker.js", import.meta.url), {
  type: "module",
});
const clientReady = connectWorker(worker);

document.getElementById("ask")?.addEventListener("click", () => {
  void ask();
});
document.getElementById("inspect")?.addEventListener("click", () => {
  void inspectLast();
});
document.getElementById("golden")?.addEventListener("click", () => {
  void runGolden();
});
document.getElementById("clear")?.addEventListener("click", () => {
  void clearLocal();
});

async function ask(): Promise<void> {
  const question = questionEl?.value.trim() ?? "";
  if (question.length === 0) {
    return;
  }
  const client = await clientReady;
  const agent = await client.create({});
  const run = agent.start(question);
  const kinds: string[] = [];
  for await (const batch of run.events()) {
    for (const event of batch.events()) {
      kinds.push(event.kind);
    }
    if (eventsEl !== null) {
      eventsEl.textContent = kinds.join("\n");
    }
  }
  const snapshot = await run.result();
  sessionStorage.setItem(LAST_SESSION_KEY, snapshot.sessionId);
  if (answerEl !== null) {
    answerEl.textContent = snapshot.text;
  }
}

async function inspectLast(): Promise<void> {
  const client = await clientReady;
  const sessionId = sessionStorage.getItem(LAST_SESSION_KEY);
  if (sessionId === null) {
    if (inspectEl !== null) {
      inspectEl.textContent = "no session yet";
    }
    return;
  }
  const snapshot = await client.inspectSession(sessionId);
  if (inspectEl !== null) {
    inspectEl.textContent = JSON.stringify(snapshot, null, 2);
  }
}

/** Run the shared golden fixture against the scripted scenario. */
async function runGolden(): Promise<void> {
  const client = await clientReady;
  const response = await fetch("./golden.json");
  const fixture = (await response.json()) as {
    entries: {
      id: string;
      question: string;
      scripted_response: string;
      must_contain: string[];
      event_kinds_expected: string[];
    }[];
  };
  const rows: string[] = [];
  for (const entry of fixture.entries) {
    const agent = await client.create({
      script: [{ text: entry.scripted_response }],
    });
    const run = agent.start(entry.question);
    const kinds: string[] = [];
    for await (const batch of run.events()) {
      for (const event of batch.events()) {
        kinds.push(event.kind);
      }
    }
    const snapshot = await run.result();
    const contentOk = entry.must_contain.every((needle) =>
      snapshot.text.includes(needle),
    );
    const kindsOk = entry.event_kinds_expected.every((kind) =>
      kinds.includes(kind),
    );
    rows.push(
      `${contentOk && kindsOk ? "PASS" : "FAIL"}  ${entry.id}` +
        (contentOk ? "" : "  [content]") +
        (kindsOk ? "" : "  [events]"),
    );
    if (goldenEl !== null) {
      goldenEl.textContent = rows.join("\n");
    }
  }
}

async function clearLocal(): Promise<void> {
  sessionStorage.removeItem(LAST_SESSION_KEY);
  await deleteIndexedDbStores();
  for (const element of [answerEl, inspectEl, eventsEl, goldenEl]) {
    if (element !== null) {
      element.textContent = "";
    }
  }
}
