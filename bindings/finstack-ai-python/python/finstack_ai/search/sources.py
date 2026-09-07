"""Coarse native source handles. Rust owns query, extraction and persistence logic."""

from __future__ import annotations

from collections.abc import Sequence
from typing import cast

from .._finstack_ai import (
    Agent,
    DocumentIndexToolset,
    JournalIndexObserver,
    MemoryExtension,
    _SearchSource,
    _TextEmbedder,
)
from ._types import (
    DocumentIndexReport,
    DocumentInput,
    GraphBuildCursor,
    GraphBuildReport,
    GraphIndexReport,
    GraphReconcileReport,
    JournalIndexReport,
    MemoryEmbeddingReport,
    ReconcileReport,
    SearchEvidence,
    SearchSourceDescriptor,
    SourceRef,
    SourceReferencePage,
    SourceResult,
)
from .config import (
    DocumentIndexConfig,
    GraphIndexConfig,
    JournalIndexConfig,
    MemorySearchConfig,
    SearchQuery,
    _encode,
)


class TextEmbedder:
    """Explicit native embedder handle; search never chooses a model implicitly."""

    def __init__(self, handle: _TextEmbedder) -> None:
        self._handle = handle

    @property
    def space(self) -> str:
        """Stable model/revision/dimensionality identity used by Semantic."""
        return self._handle.space


class HashEmbedder(TextEmbedder):
    """Deterministic offline reference embedder; not a learned semantic model.

    Args:
        dimensions: Explicit bounded vector dimensionality.
    Raises:
        RuntimeError: If dimensions are outside Rust's supported bounds.
    """

    def __init__(self, dimensions: int = 128) -> None:
        super().__init__(_TextEmbedder.hash(dimensions))


class OllamaEmbedder(TextEmbedder):
    """Opt-in native Ollama embedder; indexing/query text is sent to this endpoint.

    Args:
        base_url: Operator-configured endpoint (HTTP only to loopback IPs).
        model: Explicit installed embedding model.
        dimensions: Expected output dimensionality.
    Raises:
        RuntimeError: If endpoint or model configuration is invalid.
    """

    def __init__(self, base_url: str, model: str, dimensions: int) -> None:
        super().__init__(_TextEmbedder.ollama(base_url, model, dimensions))


class SearchSource:
    """Bound native source. Rust's SearchSource trait is the backend extension seam.

    Direct queries retain construction-time scope. Await cancellation drops native
    read futures; bounded in-flight SQLite work remains owned by its worker.
    """

    def __init__(self, handle: _SearchSource) -> None:
        self._handle = handle

    @property
    def descriptor(self) -> SearchSourceDescriptor:
        """Copy the effective capabilities, bounds and configuration digest."""
        return cast(SearchSourceDescriptor, self._handle.descriptor())

    async def search(self, query: SearchQuery, *, limit: int = 8) -> SourceResult:
        """Read bounded hits and coverage from this source.

        Args:
            query: Query data with optional journal restrictions.
            limit: Maximum retained hits.
        Returns:
            Native results with explicit source status and coverage.
        Raises:
            RuntimeError: With stable search_invalid/search_scope_denied codes.
        """
        return cast(SourceResult, await self._handle.search(_encode(query), limit))

    async def references(
        self, *, cursor: str | None = None, limit: int = 128
    ) -> SourceReferencePage:
        """Page bound source references for explicit graph rebuilds (at most 256).

        Catalog entries may be stale; index_reference revalidates exact evidence.
        Restart at the beginning after concurrent source rebuild/mutation.
        """
        return cast(SourceReferencePage, await self._handle.references(cursor, limit))

    async def read_evidence(self, reference: SourceRef) -> SearchEvidence | None:
        """Read exact live proof, or None for a deleted/corrected reference.

        Graph entities do not implement recursive evidence lookup. Source failure
        raises RuntimeError and is never represented as an absent reference.
        """
        return cast(
            SearchEvidence | None, await self._handle.read_evidence(_encode(reference))
        )


class MemorySearchSource(SearchSource):
    """Federate an existing readable memory extension without changing its scope."""

    def __init__(
        self,
        memory: MemoryExtension,
        config: MemorySearchConfig,
        *,
        embedder: TextEmbedder | None = None,
    ) -> None:
        super().__init__(
            _SearchSource.memory(
                memory, _encode(config), None if embedder is None else embedder._handle
            )
        )

    async def reconcile_embeddings(
        self, *, offset: int = 0, limit: int = 128
    ) -> MemoryEmbeddingReport:
        """Rebuild a bounded exact-scope page; continue with next_offset.

        Explicitly sends text to the configured embedder. Restart from offset zero
        after concurrent memory changes. No other scope is read or embedded.
        """
        return cast(
            MemoryEmbeddingReport,
            await self._handle.reconcile_memory_embeddings(offset, limit),
        )


class DocumentSearchSource(SearchSource):
    """Separate SQLite index over an existing agent's artifact store.

    Args:
        agent: Supplies the authorized artifact store used for exact reads.
        path: Owned index database path; never the journal database.
        config: Exact scope, parser/chunker versions and capacities.
        embedder: Optional explicit embedder; vectors require reconciliation.
    Raises:
        RuntimeError: For invalid scope/configuration, schema drift or unavailable storage.
    """

    def __init__(
        self,
        agent: Agent,
        path: str,
        config: DocumentIndexConfig,
        *,
        embedder: TextEmbedder | None = None,
    ) -> None:
        super().__init__(
            _SearchSource.documents(
                agent,
                path,
                _encode(config),
                None if embedder is None else embedder._handle,
            )
        )

    def toolset(self) -> DocumentIndexToolset:
        """Return an indexing tool effect for an agent using the same Agent.artifact_store handle."""
        return DocumentIndexToolset(self._handle)

    async def index_document(self, value: DocumentInput) -> DocumentIndexReport:
        """Idempotent trusted host rebuild; agent ingestion uses toolset() in an SDK run."""
        return cast(
            DocumentIndexReport, await self._handle.index_document(_encode(value))
        )

    async def indexed_document(
        self, input: DocumentInput
    ) -> DocumentIndexReport | None:
        """Read a current indexing receipt after validating the live source artifact."""
        return cast(
            "DocumentIndexReport | None",
            await self._handle.indexed_document(_encode(input)),
        )

    async def inputs(
        self, *, after: str | None = None, limit: int = 256
    ) -> list[tuple[str, DocumentInput]]:
        """Page the exact reconstruction catalog, bounded to 256 entries."""
        values = cast(
            list[tuple[str, DocumentInput]],
            await self._handle.document_inputs(after, limit),
        )
        return [(key, value) for key, value in values]

    async def reconcile_sources(
        self, *, after: str | None = None, limit: int = 256
    ) -> ReconcileReport:
        """Remove deleted artifacts; unavailable reads remain visible for retry."""
        return cast(
            ReconcileReport, await self._handle.reconcile_documents(after, limit)
        )

    async def reconcile_embeddings(self, *, limit: int = 128) -> int:
        """Embed a bounded pending batch with the configured embedder; opt-in egress."""
        return await self._handle.reconcile_embeddings(limit)


class JournalSearchSource(SearchSource):
    """Index only committed entries/results from explicitly authorized sessions."""

    def __init__(self, agent: Agent, path: str, config: JournalIndexConfig) -> None:
        super().__init__(_SearchSource.journal(agent, path, _encode(config)))

    async def sync_session(
        self, session_id: str, *, max_records: int = 100_000
    ) -> JournalIndexReport:
        """Backfill/increment from committed history; report pruned historical coverage."""
        return cast(
            JournalIndexReport,
            await self._handle.sync_session(_encode(session_id), max_records),
        )

    def observer(self) -> JournalIndexObserver:
        """Return metadata-only hints; explicitly await observer.drain for indexing."""
        return self._handle.observer()


class GraphSearchSource(SearchSource):
    """Opt-in SQLite property graph over exact source evidence and configured rules."""

    def __init__(
        self, path: str, config: GraphIndexConfig, sources: Sequence[SearchSource]
    ) -> None:
        super().__init__(
            _SearchSource.graph(
                path, _encode(config), [source._handle for source in sources]
            )
        )

    async def index_reference(
        self, source_id: str, reference: SourceRef
    ) -> GraphIndexReport:
        """Replace deterministic facts from one exact live source reference."""
        return cast(
            GraphIndexReport,
            await self._handle.index_reference(source_id, _encode(reference)),
        )

    async def rebuild_step(
        self, *, cursor: GraphBuildCursor | None = None, limit: int = 128
    ) -> GraphBuildReport:
        """Re-extract bounded exact evidence from configured catalogs; continue with next_cursor."""
        return cast(
            GraphBuildReport, await self._handle.rebuild_graph(_encode(cursor), limit)
        )

    async def reconcile(
        self, *, after: str | None = None, limit: int = 128
    ) -> GraphReconcileReport:
        """Revalidate source evidence; continue with next_cursor until absent."""
        return cast(
            GraphReconcileReport, await self._handle.reconcile_graph(after, limit)
        )
