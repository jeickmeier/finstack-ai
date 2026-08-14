import { createReadStream } from "node:fs";
import { readFile, stat } from "node:fs/promises";
import { createServer } from "node:http";
import { extname, join, normalize, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const root = resolve(fileURLToPath(new URL("..", import.meta.url)));
const exampleRoot = resolve(root, "../../../examples/browser-minimal");
const fixtureRoot = resolve(root, "../../../fixtures/compatibility/golden-trace");
const port = Number(process.env.FINSTACK_WASM_HARNESS_PORT ?? 4173);

const types = new Map([
  [".html", "text/html; charset=utf-8"],
  [".js", "text/javascript; charset=utf-8"],
  [".mjs", "text/javascript; charset=utf-8"],
  [".ts", "text/plain; charset=utf-8"],
  [".json", "application/json; charset=utf-8"],
  [".wasm", "application/wasm"],
  [".map", "application/json; charset=utf-8"],
]);

function readBody(request) {
  return new Promise((resolveBody, reject) => {
    const chunks = [];
    request.on("data", (chunk) => {
      chunks.push(chunk);
    });
    request.on("end", () => {
      resolveBody(Buffer.concat(chunks));
    });
    request.on("error", reject);
  });
}

function writeSseFrame(response, payload) {
  response.write(`data: ${payload}\n\n`);
}

async function handleOpenAICompatible(request, response, url) {
  await readBody(request);
  const delayMs = Number(url.searchParams.get("delay") ?? 0);
  response.writeHead(200, {
    "content-type": "text/event-stream; charset=utf-8",
    "cache-control": "no-store",
    connection: "keep-alive",
  });
  const frames = [
    JSON.stringify({
      id: "chatcmpl-scripted",
      choices: [{ delta: { content: "hel" } }],
    }),
    JSON.stringify({
      id: "chatcmpl-scripted",
      choices: [{ delta: { content: "lo" } }],
    }),
    "[DONE]",
  ];
  for (const frame of frames) {
    if (delayMs > 0) {
      await new Promise((resolveDelay) => {
        setTimeout(resolveDelay, delayMs);
      });
    }
    if (request.aborted) {
      response.end();
      return;
    }
    writeSseFrame(response, frame);
  }
  response.end();
}

const server = createServer(async (request, response) => {
  const url = new URL(request.url ?? "/", `http://127.0.0.1:${port}`);
  if (request.method === "POST" && url.pathname === "/finstack/openai") {
    try {
      await handleOpenAICompatible(request, response, url);
    } catch {
      if (!response.headersSent) {
        response.writeHead(500);
      }
      response.end();
    }
    return;
  }
  if (url.pathname.startsWith("/fixtures/golden-trace/")) {
    const relative = url.pathname.slice("/fixtures/golden-trace/".length);
    const resolved = resolve(join(fixtureRoot, normalize(relative)));
    if (!resolved.startsWith(fixtureRoot) || relative.length === 0) {
      response.writeHead(403);
      response.end("forbidden");
      return;
    }
    try {
      const info = await stat(resolved);
      if (!info.isFile()) {
        response.writeHead(404);
        response.end("not found");
        return;
      }
      response.writeHead(200, {
        "content-type": types.get(extname(resolved)) ?? "application/octet-stream",
        "cache-control": "no-store",
      });
      createReadStream(resolved).pipe(response);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
    return;
  }
  if (url.pathname === "/examples/browser-minimal" || url.pathname === "/examples/browser-minimal/") {
    url.pathname = "/examples/browser-minimal/index.html";
  }
  if (url.pathname.startsWith("/examples/browser-minimal/")) {
    const relative = url.pathname.slice("/examples/browser-minimal/".length) || "index.html";
    const resolved = resolve(join(exampleRoot, normalize(relative)));
    if (!resolved.startsWith(exampleRoot)) {
      response.writeHead(403);
      response.end("forbidden");
      return;
    }
    try {
      const info = await stat(resolved);
      if (!info.isFile()) {
        response.writeHead(404);
        response.end("not found");
        return;
      }
      const headers = {
        "content-type": types.get(extname(resolved)) ?? "application/octet-stream",
        "cache-control": "no-store",
      };
      if (extname(resolved) === ".js") {
        const source = rewriteExampleModule(await readFile(resolved, "utf8"));
        response.writeHead(200, headers);
        response.end(source);
        return;
      }
      response.writeHead(200, headers);
      createReadStream(resolved).pipe(response);
    } catch {
      response.writeHead(404);
      response.end("not found");
    }
    return;
  }
  const relative = url.pathname === "/" ? "/harness.html" : url.pathname;
  const resolved = resolve(join(root, normalize(relative)));
  if (!resolved.startsWith(root)) {
    response.writeHead(403);
    response.end("forbidden");
    return;
  }
  try {
    const info = await stat(resolved);
    if (!info.isFile()) {
      response.writeHead(404);
      response.end("not found");
      return;
    }
    response.writeHead(200, {
      "content-type": types.get(extname(resolved)) ?? "application/octet-stream",
      "cache-control": "no-store",
    });
    createReadStream(resolved).pipe(response);
  } catch {
    response.writeHead(404);
    response.end("not found");
  }
});

function rewriteExampleModule(source) {
  return source
    .replaceAll(
      'from "@finstack/ai/adapters/indexeddb"',
      'from "/dist/adapters/indexeddb.js"',
    )
    .replaceAll(
      'from "@finstack/ai/adapters/openai-compatible"',
      'from "/dist/adapters/openai-compatible.js"',
    )
    .replaceAll('from "@finstack/ai/worker"', 'from "/dist/worker.js"')
    .replaceAll('from "@finstack/ai"', 'from "/dist/index.js"');
}

server.listen(port, "127.0.0.1", () => {
  process.stdout.write(`serving ${root} on http://127.0.0.1:${port}\n`);
});
