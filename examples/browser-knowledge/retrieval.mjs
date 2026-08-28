/**
 * TypeScript-side retrieval toolset for the browser knowledge example.
 *
 * Plain browser-executable ESM (no build step) so the example worker, the
 * demo page, and the Playwright golden spec all import the one module —
 * single-sourcing the corpus, the tool schema, and the ranking. Types ride
 * along in `retrieval.d.mts`.
 *
 * Deterministic on purpose: plain term scoring, no dependencies, stable
 * tie-breaks. This is the host-adapter seam demo (`JsToolset`), not a
 * search engine.
 */

/**
 * In-page corpus: excerpts of the knowledge agent's bundled self-docs
 * (`apps/finstack-knowledge/docs/`).
 */
export const CORPUS = [
  {
    id: "architecture",
    title: "finstack-ai architecture",
    body:
      "finstack-ai is a deterministic agent microkernel. The runtime owns six " +
      "ports: model, tool, context, middleware, observer, and journal. The SDK " +
      "composes them; extensions provide trusted native implementations.",
  },
  {
    id: "sessions",
    title: "Sessions, lanes, and the event stream",
    body:
      "A session is a durable conversation owned by a journal store. Lanes are " +
      "independent ordered histories; main is the default lane. Every surface " +
      "consumes the same RunEventKind stream derived from committed records.",
  },
  {
    id: "memory",
    title: "Memory",
    body:
      "The memory extension gives the agent durable recall across sessions: a " +
      "store, a remember toolset, a recall context provider, and a capture " +
      "observer. Recall is keyword ranked and budget bounded.",
  },
  {
    id: "ingestion",
    title: "Document ingestion",
    body:
      "The document toolset converts pdf, docx, xlsx, pptx, and csv documents " +
      "to Markdown. A scanned PDF succeeds with requires_ocr true. The ingest " +
      "middleware rewrites attachments into model-visible Markdown.",
  },
];

/**
 * Cached tool schema for the `search_corpus` tool (the `JsToolsetOptions`
 * `tools` entry).
 */
export const RETRIEVAL_TOOL = {
  id: "know.search-corpus",
  model_name: "search_corpus",
  title: "Search corpus",
  description:
    "Search the in-page document corpus. Returns ranked excerpts with doc ids for citation.",
  input_schema: {
    additionalProperties: false,
    properties: {
      query: { type: "string" },
      top_k: { maximum: 4, minimum: 1, type: "integer" },
    },
    required: ["query"],
    type: "object",
  },
  output_schema: {
    additionalProperties: false,
    properties: {
      excerpts: {
        items: {
          additionalProperties: false,
          properties: {
            doc_id: { type: "string" },
            excerpt: { type: "string" },
            score: { type: "number" },
            title: { type: "string" },
          },
          required: ["doc_id", "title", "excerpt", "score"],
          type: "object",
        },
        type: "array",
      },
    },
    required: ["excerpts"],
    type: "object",
  },
  execution: "parallel",
  side_effect: "read_only",
  retry_safety: "safe_to_retry",
  approval: { requirement: "not_required", reason: null, attributes: {} },
  max_result_bytes: 8192,
  metadata: {},
};

/** Constructor options for the example's `JsToolset`. */
export const RETRIEVAL_TOOLSET_OPTIONS = {
  component: "know.tools.retrieval",
  name: "browser-knowledge-retrieval",
  tools: [RETRIEVAL_TOOL],
};

/**
 * Lowercased alphanumeric tokens of length >= 3.
 *
 * @param {string} text
 * @returns {string[]}
 */
function tokenize(text) {
  return (text.toLowerCase().match(/[a-z0-9]+/g) ?? []).filter(
    (token) => token.length >= 3,
  );
}

/**
 * Rank corpus documents against a query with deterministic term scoring.
 *
 * Tokens are lowercased alphanumeric runs of length >= 3. Each body match
 * scores 1, each title match scores 2. Ties break on document id, so equal
 * inputs always produce identical rankings.
 *
 * @param {{id: string, title: string, body: string}[]} corpus
 * @param {string} query
 * @param {number} topK
 */
export function rankCorpus(corpus, query, topK) {
  const terms = tokenize(query);
  const scored = corpus
    .map((doc) => {
      const bodyTokens = tokenize(doc.body);
      const titleTokens = tokenize(doc.title);
      let score = 0;
      for (const term of terms) {
        score += bodyTokens.filter((token) => token === term).length;
        score += 2 * titleTokens.filter((token) => token === term).length;
      }
      return { doc, score };
    })
    .filter((entry) => entry.score > 0);
  scored.sort((a, b) => b.score - a.score || a.doc.id.localeCompare(b.doc.id));
  return scored.slice(0, Math.max(1, topK)).map((entry) => ({
    doc_id: entry.doc.id,
    title: entry.doc.title,
    excerpt: entry.doc.body.slice(0, 200),
    score: entry.score,
  }));
}

/**
 * Host adapter for `JsToolset`: fulfill one validated `search_corpus` call.
 *
 * @param {{id: string, title: string, body: string}[]} corpus
 */
export function createRetrievalHost(corpus) {
  return {
    /**
     * @param {unknown} _context - JSON string of the call locator.
     * @param {unknown} call - JSON string of the validated tool call.
     */
    call: async (_context, call) => {
      const parsed = typeof call === "string" ? JSON.parse(call) : call;
      const args = parsed?.call?.arguments ?? parsed?.arguments ?? {};
      const query = typeof args.query === "string" ? args.query : "";
      const topK = typeof args.top_k === "number" ? args.top_k : 2;
      if (query.length === 0) {
        return {
          output: {
            error: { code: "retrieval_invalid_arguments", message: "query required" },
          },
          is_error: true,
        };
      }
      return { output: { excerpts: rankCorpus(corpus, query, topK) }, is_error: false };
    },
  };
}
