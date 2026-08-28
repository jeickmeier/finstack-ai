/** Types for `retrieval.mjs` (kept beside the browser-executable module). */

export interface CorpusDoc {
  id: string;
  title: string;
  body: string;
}

export interface RetrievalExcerpt {
  doc_id: string;
  title: string;
  excerpt: string;
  score: number;
}

export declare const CORPUS: CorpusDoc[];
export declare const RETRIEVAL_TOOL: Record<string, unknown>;
export declare const RETRIEVAL_TOOLSET_OPTIONS: {
  component: string;
  name: string;
  tools: unknown[];
};
export declare function rankCorpus(
  corpus: CorpusDoc[],
  query: string,
  topK: number,
): RetrievalExcerpt[];
export declare function createRetrievalHost(corpus: CorpusDoc[]): {
  call(
    context: unknown,
    call: unknown,
  ): Promise<{ output: object; is_error: boolean }>;
};
