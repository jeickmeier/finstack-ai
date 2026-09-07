"""Typed copies of Rust results; modifying these values never mutates a source."""

from typing import Literal, NotRequired, TypeAlias, TypedDict

from ..artifacts import ArtifactReference
from .config import LexicalKind, ScopeMapping

Sensitivity: TypeAlias = Literal[
    "public", "internal", "confidential", "secret", "credential"
]
SourceStatus: TypeAlias = Literal[
    "completed", "unsupported", "unavailable", "truncated"
]


class ScopeData(TypedDict):
    """Complete authority preserved in every citation."""

    tenant: str
    user: str | None
    agent: str | None
    workspace: str | None


class MemoryRef(TypedDict):
    """Live memory identifier."""

    kind: Literal["memory"]
    id: str


class ArtifactChunkRef(TypedDict):
    """Immutable scoped artifact, chunk ordinal and processing fingerprint."""

    kind: Literal["artifact_chunk"]
    artifact: ArtifactReference
    ordinal: int
    chunker: str


class JournalSpanRef(TypedDict):
    """Committed entry and retained record identifiers; snapshots may omit spans."""

    kind: Literal["journal_span"]
    session: str
    lane: str
    entry: str
    first_record: str | None
    last_record: str | None


class EntityRef(TypedDict):
    """Deterministic entity identity within the complete graph scope."""

    kind: Literal["entity"]
    id: str


SourceRef: TypeAlias = MemoryRef | ArtifactChunkRef | JournalSpanRef | EntityRef


class SearchCitation(TypedDict):
    """Exact scoped source proof for derived facts."""

    source: str
    reference: SourceRef
    scope: ScopeData
    content_digest: str


class SearchProvenance(TypedDict):
    """Bounded locators and source proofs, independent of relevance."""

    scope: ScopeData
    content_digest: str
    locators: list[str]
    citations: list[SearchCitation]


class SearchHit(TypedDict):
    """Source-ranked match with bounded untrusted preview and exact citation."""

    source: str
    reference: SourceRef
    score: int
    preview: str
    entity_label: str | None
    sensitivity: Sensitivity
    provenance: SearchProvenance


class SearchEvidence(TypedDict):
    """Live source text, bounded to 64 KiB; complete explicitly records truncation."""

    hit: SearchHit
    text: str
    complete: bool


class LexicalData(TypedDict):
    kind: Literal["lexical"]
    config: LexicalKind


class SemanticSpace(TypedDict):
    space: str


class SemanticData(TypedDict):
    kind: Literal["semantic"]
    config: SemanticSpace


class GraphQueryData(TypedDict):
    kind: Literal["entity", "neighborhood", "path"]
    target: NotRequired[str]
    depth: NotRequired[int]
    max_nodes: NotRequired[int]
    max_edges: NotRequired[int]


class GraphData(TypedDict):
    kind: Literal["graph"]
    config: GraphQueryData


StrategyData: TypeAlias = LexicalData | SemanticData | GraphData


class SourceResult(TypedDict):
    """One source's results, failure status and historical coverage."""

    hits: list[SearchHit]
    status: SourceStatus
    examined: int
    historical_complete: bool
    reasons: list[str]


class SourceOutcome(TypedDict):
    """Every attempted leg remains visible, including unavailable/unsupported legs."""

    source: str
    strategy: StrategyData
    status: SourceStatus
    examined: int
    historical_complete: bool
    reasons: list[str]


class FusedEvidence(TypedDict):
    """Per-leg rank, normalized contribution and original source provenance."""

    strategy: StrategyData
    rank: int
    source_score: int
    weight_micros: int
    contribution: int
    provenance: SearchProvenance
    sensitivity: Sensitivity


class FusedHit(TypedDict):
    """Deduplicated result retaining every contributing leg and highest sensitivity."""

    source: str
    reference: SourceRef
    score: int
    preview: str
    entity_label: str | None
    sensitivity: Sensitivity
    evidence: list[FusedEvidence]


class SearchResponse(TypedDict):
    """Fused hits and all leg outcomes; zero successful legs raises RuntimeError."""

    hits: list[FusedHit]
    outcomes: list[SourceOutcome]
    results_truncated: bool


class SearchSourceDescriptor(TypedDict):
    """Effective native capabilities and configuration fingerprint."""

    source_id: str
    kind: str
    lexical: list[LexicalKind]
    semantic_spaces: list[str]
    evidence_lookup: bool
    graph: bool
    durable: bool
    scope_mapping: ScopeMapping
    limits: dict[str, int]
    configuration_digest: str


class ArtifactScope(TypedDict):
    """Exact existing artifact-store read authority."""

    tenant_scope: str
    session_id: str
    run_id: NotRequired[str | None]
    sensitivity: Sensitivity


class DocumentInput(TypedDict):
    """Trusted host maintenance input; agents ingest through DocumentIndexToolset."""

    artifact_scope: ArtifactScope
    artifact: ArtifactReference


class DocumentIndexReport(TypedDict):
    """Idempotent extraction receipt, including explicit OCR/truncation state."""

    artifact: ArtifactReference
    chunker_digest: str
    parsed_digest: str
    chunks: int
    status: Literal["indexed", "ocr_required", "truncated"]
    requires_ocr: bool
    truncated: bool
    page_count: int | None


class ReconcileReport(TypedDict):
    """Bounded artifact liveness reconciliation; continue with next_after."""

    examined: int
    removed: int
    unavailable: int
    truncated: bool
    next_after: str | None


class JournalIndexReport(TypedDict):
    """Committed backfill/hint progress with snapshot coverage recorded."""

    session: str
    examined: int
    indexed: int
    next_sequence: int
    complete: bool
    historical_complete: bool
    used_load: bool


class GraphIndexReport(TypedDict):
    """Facts supported by one exact live reference after replacement."""

    source_key: str
    entities: int
    edges: int
    removed: bool
    complete: bool


class GraphReconcileReport(TypedDict):
    """Exact source revalidation progress; unavailable evidence stays unservable."""

    examined: int
    removed: int
    unavailable: int
    next_cursor: str | None


class MemoryEmbeddingReport(TypedDict):
    """Scoped embedding rebuild page; restart at zero after concurrent mutations."""

    examined: int
    total: int
    next_offset: int | None


class SourceReferencePage(TypedDict):
    """Bounded catalog candidates; read exact live evidence before deriving facts."""

    references: list[SourceRef]
    next_cursor: str | None


class GraphBuildCursor(TypedDict):
    """Position in the graph's configured evidence-source catalogs."""

    source: str
    reference: str | None


class GraphBuildReport(TypedDict):
    """Bounded graph rebuild progress; unavailable catalogs never imply full coverage."""

    indexed: int
    removed: int
    unavailable: int
    next_cursor: GraphBuildCursor | None
