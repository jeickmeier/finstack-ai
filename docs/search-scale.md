# SQLite search operating envelope

The supported local envelope is 100,000 document chunks and 10,000 retained
memory records. Exact vectors remain the implementation; no ANN backend is
included. These limits bound work, but do not promise a latency SLA or certify
semantic relevance. Larger embedding dimensions, longer evidence, concurrent
clients and remote embedders require separate measurement.

## Reproduce

Run `mise run bench-search` for a fresh deterministic corpus. It builds the
`release-fast` native example before measurement and writes
`target/search-benchmark.json`. For a retained corpus and an isolated query pass:

```sh
mise exec -- python scripts/benchmarks/search.py \
  --corpus-dir /tmp/finstack-search-benchmark --output target/search-first.json
mise exec -- python scripts/benchmarks/search.py \
  --corpus-dir /tmp/finstack-search-benchmark --reuse --output target/search-reuse.json
```

The corpus directory must be empty for the first command. The launcher also
accepts `--chunks`, `--memories`, `--journal-entries` and `--samples`; the native
runner rejects values outside its supported bounds. Each measured pass is a
separate native process. Unix `getrusage(RUSAGE_CHILDREN)` reports peak resident
memory; compiler RSS is excluded. Unsupported platforms fail instead of
substituting a zero measurement.

## Fixture and method

Measurements were collected on an Apple M5 Max (18 logical CPUs, 128 GiB RAM),
Darwin 25.5.0 arm64, with the repository's pinned Rust toolchain and `release-fast` (optimization level 3, no LTO, 16 codegen units).
The synthetic corpus contains 1,000 Markdown artifacts producing exactly
100,000 heading/paragraph-aware chunks with the default 4,096-character target
and 512-character overlap; 10,000 memories with 512-byte inline bodies; 1,000
committed journal entries; and a 10,000-entity cyclic company graph with scoped
memory evidence. Document bodies are real stored artifacts, not short repeated
index-only rows. The explicit offline HashEmbedder has 64 dimensions; it
exercises storage and exact cosine computation, not language-model quality.

Each query family uses three warmups and 30 measured requests, returning at most
eight hits with 512-character previews. The report includes all sorted samples,
nearest-rank p95/p99 and the lower central p50. Hybrid runs four memory/document
lexical/semantic legs through the real fusion engine. Graph queries perform
bounded two-hop neighborhoods and validate original source evidence. The harness
fails on an empty result, lost source, wrong scope or excessive source preview.
A `truncated` outcome here normally means a full top-k page; it does not mean the
benchmark discarded an unavailable source. Journal backfill builds five fresh
indexes from committed history and reports every duration.

## Measured baseline and optimization

The [original full run](benchmarks/search-scale-before.json) was recorded before
changing the algorithms. The [query-only run after optimization](benchmarks/search-scale-query-after.json)
reuses that exact on-disk corpus. They are local development measurements, not
controlled throughput benchmarks. Do not interpret small timing differences as
regressions or wins.

| Query | Baseline p50 / p95 (ms) | After p50 / p95 (ms) |
|---|---:|---:|
| Memory lexical | 3.09 / 3.56 | 3.06 / 4.13 |
| Memory exact semantic | 106.89 / 108.91 | 99.97 / 116.11 |
| Document lexical | 173.76 / 180.04 | 188.27 / 208.21 |
| Document exact semantic | 317.19 / 326.21 | 302.40 / 317.21 |
| Four-leg hybrid | 519.52 / 535.37 | 483.90 / 510.32 |
| Journal lexical | 0.71 / 1.03 | 0.72 / 0.80 |
| Graph neighborhood | 1107.59 / 1126.89 | 31.94 / 32.78 |

The graph seed query previously sorted/scanned the whole alias catalog for each
page. Ordered scope/entity and entity/alias indexes plus an explicit lower bound
on the entity cursor remove that repeated work. Graph schema v2 migrates v1
atomically and preserves evidence and ordering. Memory exact search now retains
only top-k records in a heap, with deterministic score/ID ties, instead of
materializing all matches.

The isolated memory-query process fell from 205.06 MiB peak RSS to 13.19 MiB.
The baseline full process peaked at 222.92 MiB during construction and queries;
the optimized reuse process peaked at 35.38 MiB during queries only. Those two
full-process figures cover different work and are **not** a like-for-like memory
improvement claim. The memory-only processes both query existing stores.

Baseline construction took 47.02 seconds for memory, 0.74 seconds for memory
vectors, 21.85 seconds for document indexing and 252.30 seconds for document
vectors. Document lexical rebuild from source artifacts took 8.47 seconds;
this excludes semantic regeneration. Graph extraction/build took 169.83 seconds.
Journal backfill median/p95 was 63.96/67.23 ms for 1,000 committed entries.
The first run's graph build timing predates the lookup-index optimization.

After optimization, the document index occupied 491.21 MiB, memory 38.48 MiB,
graph 51.20 MiB, journal search index 1.47 MiB and source artifacts 361.26 MiB.
Exact file sizes and WAL/SHM files are included in the JSON. Index size excludes
the separate authoritative journal, artifacts and independently rebuilt copies.

## Final build with all limits enabled

The [final fresh-corpus run](benchmarks/search-scale-final.json) includes the
aggregate memory-vector byte cap and pre-dispatch semantic admission. It reruns
construction, reconstruction and queries at the same 100,000/10,000 scale.
Peak RSS was **43.19 MiB** for the full native process and **13.09 MiB** for a
separate memory-query process. Repository validation was running on the same
host during this measurement; its timings are a practical development sample,
not an isolated optimization comparison.

| Query | Final p50 / p95 (ms) |
|---|---:|
| Memory lexical | 2.68 / 2.84 |
| Memory exact semantic | 98.44 / 99.22 |
| Document lexical | 232.03 / 463.05 |
| Document exact semantic | 318.03 / 377.99 |
| Four-leg hybrid | 486.36 / 495.93 |
| Journal lexical | 0.70 / 0.74 |
| Graph neighborhood | 33.47 / 34.14 |

Memory construction/vectors took 40.23/5.28 seconds; document
construction/vectors took 19.80/275.46 seconds. The new atomic memory-vector
admission computes retained bytes inside each write transaction, so its full
rebuild cost is included rather than hidden. Document lexical reconstruction
took 8.32 seconds, graph construction 200.06 seconds and journal backfill
median/p95 61.03/81.21 ms. All query families retained successful coverage;
full top-k pages remain explicitly truncated.

The final graph database occupied 51.60 MiB and the memory database 38.35 MiB,
with a further 4.51 MiB of memory WAL/SHM at measurement time. Document and
journal-index sizes were unchanged. Compare databases plus their recorded
sidecars, not just the main SQLite file, when budgeting disk space.

## Enforced bounds and deployment implications

| Resource | Default | Enforcement |
|---|---:|---|
| Source scan | 100,000 records | Query admission and bounded source scans |
| Memory retention | 10,000 records; 16 MiB inline bodies | Store writes, including retained lifecycle records |
| Results | 256 per leg/final response | Source and fusion admission/retention; exact vectors keep top-k |
| Preview | 512 Unicode scalar values | Source rendering and result validation |
| Embedding storage | 256 MiB | Document indexing; atomic memory-store aggregate vector writes |
| Memory semantic scan/rebuild | 256 MiB of live records × dimensions × 4 | Admission before embedder dispatch |
| Graph traversal | 2 hops, 100 nodes, 1,000 edges | Explicit traversal counters and truncated outcomes |
| Graph retained state | 10,000 entities, 100,000 edges | Atomic source replacement and configured evidence bounds |
| Hybrid fan-out | 32 legs | Validated plan, bounded concurrency and source timeouts |

At 100,000 chunks, 64-dimensional f32 vectors alone require about 24.4 MiB;
768 dimensions require about 293 MiB and exceed the default embedding ceiling.
Raising the configurable ceiling is an explicit deployment decision, not a claim
that this benchmark validated larger vectors. Memory's store ceiling covers all
retained scopes and embedding spaces. Its adapter also conservatively budgets
all live records in the bound scope, including records whose vectors are pending.

SQLite lexical matching, exact vector scans, graph seed enumeration and live
source checks remain corpus-dependent. The fixture does not establish behavior
under sustained concurrent ingest/query traffic, cold OS caches, remote storage,
remote embedding latency or 100,000-entry journal histories. Re-run with a
representative workload before making a latency commitment. Source correction,
deletion, authorization and rebuild correctness are covered separately by
`mise run test-search` and `test-search-python`.
