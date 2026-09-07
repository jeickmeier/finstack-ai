"""Immutable configuration converted to canonical Rust search contracts."""

from __future__ import annotations

import json
from collections.abc import Sequence
from dataclasses import dataclass, field, fields, is_dataclass
from typing import Literal, TypeAlias

ScopeMapping: TypeAlias = Literal["exact", "restrict_to_bound"]
LexicalKind: TypeAlias = Literal["keyword", "bm25", "literal", "regex"]


def _wire(value: object) -> object:
    if isinstance(value, (Lexical, Semantic, Graph)):
        return value._wire()
    if is_dataclass(value) and not isinstance(value, type):
        return {f.name: _wire(getattr(value, f.name)) for f in fields(value)}
    if isinstance(value, (tuple, list)):
        return [_wire(v) for v in value]
    if isinstance(value, dict):
        return {k: _wire(v) for k, v in value.items()}
    return value


def _encode(value: object) -> str:
    return json.dumps(
        _wire(value), ensure_ascii=False, allow_nan=False, separators=(",", ":")
    )


@dataclass(frozen=True)
class SearchScope:
    """Complete host authority; each optional dimension narrows the bound scope."""

    tenant: str
    user: str | None = None
    agent: str | None = None
    workspace: str | None = None


@dataclass(frozen=True)
class SearchLimits:
    """Finite per-query ceilings, validated in Rust before source fan-out."""

    max_query_bytes: int = 4096
    max_pattern_bytes: int = 1024
    max_scan_records: int = 100_000
    max_results: int = 256
    max_preview_chars: int = 512
    max_legs: int = 32
    max_embedding_bytes: int = 268_435_456
    max_graph_nodes: int = 100
    max_graph_edges: int = 1000


@dataclass(frozen=True)
class Lexical:
    """Offline token, BM25, literal or bounded Rust regex strategy."""

    config: LexicalKind = "bm25"

    def _wire(self) -> object:
        return {"kind": "lexical", "config": self.config}


@dataclass(frozen=True)
class Semantic:
    """Exact-vector search in an explicitly configured embedder space."""

    space: str

    def _wire(self) -> object:
        return {"kind": "semantic", "config": {"space": self.space}}


@dataclass(frozen=True)
class Graph:
    """Entity lookup or bounded neighborhood/directed shortest path.

    The query text supplies the source label; path additionally requires target.
    Traversal limits include seeds, cycle nodes and examined back edges.
    """

    kind: Literal["entity", "neighborhood", "path"] = "entity"
    target: str | None = None
    depth: int = 2
    max_nodes: int = 100
    max_edges: int = 1000

    def _wire(self) -> object:
        config: dict[str, object] = {"kind": self.kind}
        if self.kind != "entity":
            config.update(
                depth=self.depth, max_nodes=self.max_nodes, max_edges=self.max_edges
            )
        if self.kind == "path":
            config["target"] = self.target
        return {"kind": "graph", "config": config}


SearchStrategy: TypeAlias = Lexical | Semantic | Graph


@dataclass(frozen=True)
class JournalFilter:
    """Optional session/lane subset and inclusive UTC timestamp bounds.

    ISO 8601 timestamps use the kernel wire format. Filters never grant access
    to sessions absent from JournalIndexConfig.sessions.
    """

    sessions: Sequence[str] = ()
    lanes: Sequence[str] = ()
    after: str | None = None
    before: str | None = None


@dataclass(frozen=True)
class SearchQuery:
    """Direct source query; scope remains frozen in the source handle."""

    text: str
    strategy: SearchStrategy = field(default_factory=Lexical)
    journal: JournalFilter = field(default_factory=JournalFilter)


@dataclass(frozen=True)
class SearchRequest:
    """Federated query data, with optional source/strategy selection."""

    text: str
    strategy: SearchStrategy | None = None
    sources: Sequence[str] | None = None
    limit: int | None = None
    journal: JournalFilter = field(default_factory=JournalFilter)


@dataclass(frozen=True)
class HybridLeg:
    """One source strategy and relative weight in millionths (1..1,000,000)."""

    source: str
    strategy: SearchStrategy = field(default_factory=Lexical)
    weight_micros: int = 1_000_000


@dataclass(frozen=True)
class Rrf:
    """Weighted reciprocal rank fusion; default smoothing k is 60."""

    k: int = 60
    kind: Literal["rrf"] = field(default="rrf", init=False)


@dataclass(frozen=True)
class WeightedSum:
    """Normalize each leg to [0,1] before combining weighted scores."""

    kind: Literal["weighted_sum"] = field(default="weighted_sum", init=False)


@dataclass(frozen=True)
class HybridPlan:
    """Explicit legs; semantic and graph retrieval never appear implicitly."""

    legs: Sequence[HybridLeg]
    fusion: Rrf | WeightedSum = field(default_factory=Rrf)


@dataclass(frozen=True)
class GraphExpansion:
    """Opt-in, evidenced query expansion; skipped for literal/regex/graph queries."""

    source: str
    depth: int = 2
    max_nodes: int = 100
    max_edges: int = 1000
    weight_micros: int = 1_000_000


@dataclass(frozen=True)
class SearchConfig:
    """Frozen federation, authenticated scope and bounded concurrency/deadlines."""

    scope: SearchScope
    default_plan: HybridPlan
    graph_expansion: GraphExpansion | None = None
    limits: SearchLimits = field(default_factory=SearchLimits)
    max_concurrency: int = 4
    source_timeout_ms: int = 10_000


@dataclass(frozen=True)
class MemorySearchConfig:
    """Existing memory adapter; scope must equal the readable MemoryExtension."""

    source_id: str
    scope: SearchScope
    scope_mapping: ScopeMapping = "exact"
    limits: SearchLimits = field(default_factory=SearchLimits)


@dataclass(frozen=True)
class ChunkerConfig:
    """Heading/paragraph chunking in Unicode characters, version 1."""

    version: int = 1
    target_chars: int = 4096
    overlap_chars: int = 512


@dataclass(frozen=True)
class DocumentLimits:
    """Existing parser input/output/page ceilings; OCR remains explicit."""

    max_input_bytes: int = 4_194_304
    max_output_bytes: int = 1_048_576
    max_pages: int = 500


@dataclass(frozen=True)
class DocumentIndexConfig:
    """Separate derived SQLite index with exact artifact authority and chunk limits."""

    source_id: str
    scope: SearchScope
    scope_mapping: ScopeMapping = "exact"
    chunker: ChunkerConfig = field(default_factory=ChunkerConfig)
    limits: SearchLimits = field(default_factory=SearchLimits)
    parse_limits: DocumentLimits = field(default_factory=DocumentLimits)
    max_chunks: int = 100_000
    max_documents: int = 10_000


@dataclass(frozen=True)
class JournalIndexConfig:
    """Committed history over an explicit host-owned list of authorized sessions."""

    source_id: str
    scope: SearchScope
    sessions: Sequence[str]
    scope_mapping: ScopeMapping = "exact"
    limits: SearchLimits = field(default_factory=SearchLimits)
    max_entries: int = 100_000
    max_entry_bytes: int = 65_536
    sensitivity: Literal["public", "internal", "confidential", "secret"] = (
        "confidential"
    )
    io_timeout_ms: int = 10_000


@dataclass(frozen=True)
class EntityRule:
    """Bounded Rust regex with named label capture and optional canonical alias."""

    kind: str
    pattern: str
    canonical_label: str | None = None


@dataclass(frozen=True)
class EdgeKind:
    """Directed relationship kind and the entity kinds at both endpoints."""

    kind: str
    source_kind: str
    target_kind: str


@dataclass(frozen=True)
class EdgeRule:
    """Bounded Rust regex with named source and target captures."""

    kind: str
    pattern: str


@dataclass(frozen=True)
class GraphVocabulary:
    """Versioned deterministic extraction vocabulary; no model extraction."""

    entity_kinds: Sequence[str]
    edge_kinds: Sequence[EdgeKind]
    entity_rules: Sequence[EntityRule]
    edge_rules: Sequence[EdgeRule]
    version: int = 1


@dataclass(frozen=True)
class GraphIndexConfig:
    """Scoped SQLite property graph with explicit extraction and traversal limits."""

    source_id: str
    scope: SearchScope
    vocabulary: GraphVocabulary
    scope_mapping: ScopeMapping = "exact"
    limits: SearchLimits = field(default_factory=SearchLimits)
    max_sources: int = 100_000
    max_entities: int = 10_000
    max_edges: int = 100_000
    max_entities_per_source: int = 128
    max_edges_per_source: int = 256
    max_support_per_item: int = 64
    max_evidence_reads: int = 256
    source_timeout_ms: int = 1000
