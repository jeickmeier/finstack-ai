import { connectWorker } from "@finstack/ai/worker";
import { deleteIndexedDbStores } from "@finstack/ai/adapters/indexeddb";

const resultEl = document.getElementById("result");
const inspectEl = document.getElementById("inspect-panel");
const eventsEl = document.getElementById("events");
const runButton = document.getElementById("run");
const inspectButton = document.getElementById("inspect");
const clearButton = document.getElementById("clear");

const LAST_SESSION_KEY = "finstack-ai-experimental-last-session";

const worker = new Worker(new URL("./worker.js", import.meta.url), {
  type: "module",
});
const clientReady = connectWorker(worker);

runButton?.addEventListener("click", () => {
  void runSession();
});
inspectButton?.addEventListener("click", () => {
  void inspectLast();
});
clearButton?.addEventListener("click", () => {
  void clearLocal();
});

async function runSession(): Promise<void> {
  const client = await clientReady;
  const agent = await client.create({ scenario: "scripted" });
  const run = agent.start("hello");
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
  if (resultEl !== null) {
    resultEl.textContent = snapshot.text;
  }
}

async function inspectLast(): Promise<void> {
  const client = await clientReady;
  const sessionId = sessionStorage.getItem(LAST_SESSION_KEY);
  if (sessionId === null) {
    if (inspectEl !== null) {
      inspectEl.textContent = "no lastSessionId locator";
    }
    return;
  }
  const snapshot = await client.inspectSession(sessionId);
  if (inspectEl !== null) {
    inspectEl.textContent = JSON.stringify(snapshot, null, 2);
  }
}

async function clearLocal(): Promise<void> {
  sessionStorage.removeItem(LAST_SESSION_KEY);
  await deleteIndexedDbStores();
  if (resultEl !== null) {
    resultEl.textContent = "cleared";
  }
  if (inspectEl !== null) {
    inspectEl.textContent = "cleared";
  }
  if (eventsEl !== null) {
    eventsEl.textContent = "";
  }
}
