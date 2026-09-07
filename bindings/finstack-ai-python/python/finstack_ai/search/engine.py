"""Agent-facing federation, entirely implemented by the native Rust engine."""

from collections.abc import Sequence
from typing import cast

from .._finstack_ai import SearchContextProvider, SearchToolset, _SearchEngine
from ._types import SearchResponse
from .config import SearchConfig, SearchRequest, _encode
from .sources import SearchSource


class SearchEngine:
    """Bind sources once to immutable authority, strategy weights and resource limits.

    Args:
        config: Exact host scope, default plan and optional graph expansion.
        sources: Native source handles with unique source namespaces.
    Raises:
        RuntimeError: For invalid plans, duplicate identities or scope conflicts.
    """

    def __init__(self, config: SearchConfig, sources: Sequence[SearchSource]) -> None:
        self._handle = _SearchEngine(
            _encode(config), [source._handle for source in sources]
        )

    async def search(self, request: SearchRequest) -> SearchResponse:
        """Search and fuse all selected legs, preserving unavailable/partial coverage.

        Args:
            request: Query data; model arguments cannot supply authority.
        Returns:
            Typed results with all source outcomes and bounded untrusted previews.
        Raises:
            RuntimeError: search_no_successful_sources retains outcomes in context;
                invalid queries and authorization failures precede fan-out.
        """
        return cast(SearchResponse, await self._handle.search(_encode(request)))

    def toolset(self) -> SearchToolset:
        """One search tool accepted by every agent factory."""
        return self._handle.toolset()

    def context_provider(self, *, max_hits: int = 8) -> SearchContextProvider:
        """Global recall; register instead of the standalone memory recall provider."""
        return self._handle.context_provider(max_hits)
