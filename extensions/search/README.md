# Scoped search

`finstack-ai-search-core` defines `SearchSource`, complete scopes, typed citations,
query limits, source coverage, and deterministic fusion. `finstack-ai-search`
composes sources into `SearchEngine`, the `search` tool, and optional
`SearchContextProvider`. Backends own their storage behind `SearchSource`.

Bind a complete authenticated `SearchScope` when constructing every source and
engine. `ScopeMapping::Exact` is the default. `RestrictToBound` can fill omitted
restrictions with already bound values; it never discards a requested dimension
or accepts a conflicting restriction. All selected legs authorize before fan-out.
Model arguments cannot select tenant, user, agent, workspace, or backend paths.

Use `HybridPlan` to select source/strategy pairs and integer weights. Lexical
retrieval is offline. Semantic retrieval needs an explicit `TextEmbedder` and its
space ID; a remote embedder is a host-owned egress decision. Reciprocal rank
fusion defaults to `k = 60`. Weighted sum normalizes each leg before applying
weights. Deduplication uses source namespace plus typed reference, retaining
contributing evidence and the highest sensitivity.

Every response reports completed, unsupported, unavailable, or truncated source
outcomes. Zero successful sources returns `search_no_successful_sources` with the
outcomes; the tool marks that result as an error. Context recall emits untrusted
references plus coverage metadata, including any hits omitted to fit its budget.
In a global composition register global recall in place of memory recall.
Standalone memory recall remains available.

## Sources and ownership

- `finstack-ai-memory` with feature `search` adapts an existing memory store.
  Lexical ordering remains backend-defined; cross-leg raw scores are never added.
  Correction/forget operations invalidate vectors atomically. Semantic coverage
  reports pending or unknown vectors instead of claiming complete empty results.
- `finstack-ai-index-documents` owns a separate SQLite database at schema version 1.
  Register `DocumentIndexToolset` and invoke `index_document` through an agent run.
  The runtime commits the effect before indexing. Receipts key the complete artifact
  reference, search scope, and versioned extraction/chunking configuration; repeating
  a call returns the same receipt. Parser failures/capacity failures write no rows.

Document chunking targets 4,096 Unicode scalar values with up to 512 overlap,
preferentially following headings and paragraphs. Citations retain exact parsed
Markdown character offsets and bounded headings; page positions are not invented.
The existing parser preserves OCR-required and truncated extraction status. No OCR
runs. Parsing plus chunker configuration contributes to the receipt fingerprint.

The artifact store remains authoritative and owns pinning/GC. The document index
never pins source artifacts. Keep durable artifact ownership in the application.
Query results verify artifact liveness and integrity before exposing previews;
missing sources are removed from derived tables. `reconcile_sources` sweeps bounded
catalog pages. Save explicit `DocumentInput` references, or page `inputs` across
processing versions, to rebuild a fresh index with `index_document` from trusted
host code. This creates no global artifact discovery API.

`reconcile_embeddings` is separate bounded derived-data maintenance. Lexical
retrieval remains usable while embeddings are unavailable. The default limits are
100,000 scanned records, 256 retained results per leg, 512 preview characters, and
256 MiB stored document embedding bytes. Reduced limits are enforced and reported;
these ceilings alone are not a latency claim. Index connections have bounded job
queues and join their worker on final drop.

Native Rust implementations are T1 trusted in-process extensions. Index contents
are T5 untrusted evidence. Errors contain stable codes/reasons without backend paths
or query text. Native storage/search is not available in browser WASM. Search-core
remains target-portable. Python exposes all four native sources through `finstack_ai.search`.

Run `mise run test-search` for offline scope, fusion, source lifecycle, parser,
resource-bound, cancellation, and committed SDK effect checks. Generate owned
public inventories with `mise run write-public-api`; verify them with
`mise run check-public-api`.

## Committed journals and evidenced graphs

`finstack-ai-index-journal` owns a versioned derived SQLite index. Bind an explicit
set of authorized session IDs in `JournalIndexConfig`. The knowledge application
may enumerate its own SQLite file; `JournalStore` exposes no global discovery.
`JournalIndexObserver` queues metadata-only hints. Draining hints and
`sync_session(session, max_records)` both read committed messages, entries and
tool results. Transient token events are never indexed. Citations use committed
entry/record identities. Verified snapshot/full-load reconstruction handles stores
without usable scans/retained prefixes; unavailable historical coverage stays
explicit. Session/lane/time query filters only narrow the bound authority.

`finstack-ai-index-graph` owns a scoped SQLite property graph built from configured
entity and edge rules. Every fact retains exact source evidence. Entity lookup,
bounded neighborhoods and directed shortest paths revalidate that evidence before
returning facts. Deleted or corrected support is removed; independent support
remains. Vocabulary drift requires re-extraction. Node/edge limits, source-read
counts, timeouts and support ceilings prevent unbounded traversal, including cycles.
A graph can index memory, document and journal sources through `read_evidence`;
there is no model extraction or graph-on-graph evidence recursion.

`GraphSearchSource::rebuild_step` pages configured source catalogs and reads each
candidate's exact evidence before indexing. Continue from `next_cursor`, and
restart a pass after source catalog mutation. Pair rebuilds with `reconcile` to
remove disappeared support. Source catalogs are metadata candidates, not proof
that content is still live. Sources may explicitly reject catalog enumeration.

`GraphExpansion` is separately opt-in on the engine, defaulting to two hops and
100 entities. Explicit source/strategy selections bypass implicit expansion.
`SearchSource` remains the backend extension boundary; no remote/vector backend
plugin is created until a concrete measured workload needs one.

## Python and the knowledge application

`finstack_ai.search` exposes immutable typed configuration, native sources,
`SearchEngine`, `SearchRequest`, typed results and maintenance reports. For example,
after populating a `MemoryExtension` through its normal write tools:

```python
from finstack_ai import search as s

scope = s.SearchScope("python-local")
source = s.MemorySearchSource(memory, s.MemorySearchConfig("memory", scope))
engine = s.SearchEngine(
    s.SearchConfig(scope, s.HybridPlan([s.HybridLeg("memory")])), [source]
)
result = await engine.search(s.SearchRequest("retention", limit=8))
```

For four-source composition, use the
[knowledge application](../../apps/finstack-knowledge/README.md). It supplies one
search tool and global recall, retains bounded maintenance cursors, and exposes
maintenance failures independently of run events. See the
[capability matrix](../../docs/capabilities.md) for browser deferrals and the
[scale report](../../docs/search-scale.md) for measured latency, memory and index
size. `mise run bench-search` constructs actual deterministic source corpora and
records an optimized native baseline without provider calls or ANN.
