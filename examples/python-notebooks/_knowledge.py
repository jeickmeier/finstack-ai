"""Shared native knowledge composition for the k-track and persistent recipe.

Rust owns search/index semantics. This application binds its SQLite session catalog,
memory, artifacts, optional embedder and graph, then explicitly maintains derived
indexes between turns. Global recall replaces standalone memory recall.
"""

from __future__ import annotations

import json
from collections.abc import Iterable, Sequence
from dataclasses import dataclass, field
from pathlib import Path
from typing import Any

import finstack_ai
from finstack_ai import search as s

#: Base instruction, verbatim from ``apps/finstack-knowledge/src/compose.rs``.
BASE_INSTRUCTION = (
    "You are the finstack knowledge assistant. Answer from ingested documents, "
    "remembered facts, and the bundled self-docs; say when you do not know. "
    "Prefer citing sources by name. Use registered tools when they help."
)

#: The citations skill, verbatim identity from the Rust composition.
CITATIONS_CAPABILITY = finstack_ai.Capability(
    "finstack.know.skill.citations",
    "Citation discipline for retrieved sources",
    [
        (
            "When answering from retrieved documents or memory, name the source "
            "(document name or memory record) for each claim and quote sparingly."
        )
    ],
    activation="model",
)

#: Grounding policy entry, verbatim from ``apps/finstack-knowledge/src/compose.rs``.
GROUNDING_POLICY = (
    "knowledge-grounding",
    "Ground answers in retrieved context; never invent citations.",
)

#: Sliding-window compaction thresholds, verbatim from the Rust composition.
COMPACTION_TOKENS = (120_000, 24_000)

#: The bundled self-docs tree (the CLI materializes the same five topics).
_SELF_DOCS_DIR = (
    Path(__file__).resolve().parent.parent.parent
    / "apps"
    / "finstack-knowledge"
    / "docs"
)
_SELF_DOCS_ALLOWLIST = [
    "architecture.md",
    "sessions.md",
    "memory.md",
    "ingestion.md",
    "cli.md",
]

#: Repository-relative path of the shared golden fixture.
_GOLDEN_PATH = (
    Path(__file__).resolve().parent.parent.parent
    / "apps"
    / "finstack-knowledge"
    / "fixtures"
    / "golden.json"
)


def knowledge_memory(
    data_dir: Path, *, tenant: str = "python-local"
) -> finstack_ai.MemoryExtension:
    """Open the shared memory store at ``<data_dir>/memory.sqlite3``.

    The path matches the ``finstack-know`` CLI, so notebooks pointed at a
    CLI data directory recall the same records. ``tenant`` must equal the
    tenant scope of the runs that recall: ``"python-local"`` for direct
    ``Agent.run``/``Agent.start``, or the session's tenant scope
    (``"local"`` for CLI-created sessions) when running on a lane.
    """

    data_dir.mkdir(parents=True, exist_ok=True)
    return finstack_ai.MemoryExtension.sqlite(
        path=str(data_dir / "memory.sqlite3"),
        tenant=tenant,
        manage=True,
    )


@dataclass
class KnowledgeAgent:
    """Application composition: SDK execution plus explicit search maintenance."""

    agent: finstack_ai.Agent
    search: s.SearchEngine
    memory: s.MemorySearchSource
    documents: s.DocumentSearchSource
    journal: s.JournalSearchSource
    graph: s.GraphSearchSource | None
    observer: s.JournalIndexObserver
    sessions: Sequence[str]
    embedded: bool
    initial_maintenance: dict[str, Any] = field(default_factory=dict)
    _document_cursor: str | None = None
    _graph_cursor: str | None = None
    _build_cursor: s.GraphBuildCursor | None = None
    _memory_offset: int = 0
    _journal_offset: int = 0

    async def maintain(self, limit: int = 128) -> dict[str, Any]:
        """Run bounded native maintenance; return source failures and remaining work.

        Call between completed/cancelled turns. Each indexing/retrieval operation
        remains Rust-owned; this is host sequencing with no background task.
        """
        if not 1 <= limit <= 256:
            raise ValueError("search_maintenance_limit")
        report: dict[str, Any] = {
            "journal_records": 0,
            "graph_references": 0,
            "removed": 0,
            "embeddings": 0,
            "more": False,
            "failures": [],
        }
        try:
            for page in await self.observer.drain(1, limit):
                report["journal_records"] += page["examined"]
                report["more"] |= not page["complete"]
        except finstack_ai.FinstackError as error:
            report["failures"].append(error.code)
        budget = limit
        for _ in self.sessions:
            if budget == 0:
                report["more"] = True
                break
            session = self.sessions[self._journal_offset % len(self.sessions)]
            try:
                page = await self.journal.sync_session(session, max_records=budget)
                budget -= page["examined"]
                report["journal_records"] += page["examined"]
                if not page["complete"]:
                    report["more"] = True
                    break
            except finstack_ai.FinstackError as error:
                report["failures"].append(error.code)
            self._journal_offset = (self._journal_offset + 1) % len(self.sessions)
        try:
            documents = await self.documents.reconcile_sources(
                after=self._document_cursor, limit=limit
            )
            report["removed"] += documents["removed"]
            report["more"] |= documents["truncated"]
            self._document_cursor = documents["next_after"]
            if documents["unavailable"]:
                report["failures"].append("document_sources_unavailable")
        except finstack_ai.FinstackError as error:
            report["failures"].append(error.code)
        if self.embedded:
            try:
                memory = await self.memory.reconcile_embeddings(
                    offset=self._memory_offset, limit=limit
                )
                self._memory_offset = memory["next_offset"] or 0
                report["embeddings"] += memory["examined"]
                report["more"] |= memory["next_offset"] is not None
                count = await self.documents.reconcile_embeddings(limit=min(limit, 64))
                report["embeddings"] += count
                report["more"] |= count == min(limit, 64)
            except finstack_ai.FinstackError as error:
                report["failures"].append(error.code)
        if self.graph is not None:
            try:
                graph = await self.graph.reconcile(
                    after=self._graph_cursor, limit=limit
                )
                self._graph_cursor = graph["next_cursor"]
                report["removed"] += graph["removed"]
                report["more"] |= graph["next_cursor"] is not None
                if graph["unavailable"]:
                    report["failures"].append("graph_sources_unavailable")
                build = await self.graph.rebuild_step(
                    cursor=self._build_cursor, limit=limit
                )
                self._build_cursor = build["next_cursor"]
                report["graph_references"] += build["indexed"]
                report["removed"] += build["removed"]
                report["more"] |= build["next_cursor"] is not None
                if build["unavailable"]:
                    report["failures"].append("graph_sources_unavailable")
            except finstack_ai.FinstackError as error:
                report["failures"].append(error.code)
        report["failures"] = sorted(set(report["failures"]))
        return report


async def build_knowledge_agent(
    data_dir: Path,
    model: finstack_ai.PythonModel,
    *,
    tenant: str = "python-local",
    memory: finstack_ai.MemoryExtension | None = None,
    observers: Iterable[Any] = (),
    output_type: type[Any] | None = None,
    sessions: Sequence[str] | None = None,
    embedder: s.TextEmbedder | None = None,
    graph_vocabulary: s.GraphVocabulary | None = None,
) -> KnowledgeAgent:
    """Compose the existing knowledge application, with lexical search by default.

    The application explicitly enumerates its own local session IDs when sessions
    is omitted. Graph and semantic retrieval require opt-in configuration. Use
    the returned .agent for SDK calls and await .maintain() between turns; failures
    and remaining work are returned, while queries always expose source coverage.
    """
    data_dir.mkdir(parents=True, exist_ok=True)
    memory = memory or knowledge_memory(data_dir, tenant=tenant)
    journal_path = data_dir / "journal.sqlite3"
    owner = await finstack_ai.Agent.from_python(
        model,
        sqlite_path=str(journal_path),
        artifact_path=str(data_dir / "artifacts"),
        document_tools=False,
    )
    authorized = tuple(
        await finstack_ai.sqlite_session_ids(str(journal_path))
        if sessions is None
        else sessions
    )
    scope = s.SearchScope(tenant)
    memory_source = s.MemorySearchSource(
        memory, s.MemorySearchConfig("memory", scope), embedder=embedder
    )
    documents = s.DocumentSearchSource(
        owner,
        str(data_dir / "documents.sqlite3"),
        s.DocumentIndexConfig("documents", scope),
        embedder=embedder,
    )
    journal = s.JournalSearchSource(
        owner,
        str(data_dir / "journal-index.sqlite3"),
        s.JournalIndexConfig("journal", scope, authorized),
    )
    hints = journal.observer()
    sources: list[s.SearchSource] = [memory_source, documents, journal]
    graph = (
        None
        if graph_vocabulary is None
        else s.GraphSearchSource(
            str(data_dir / "graph.sqlite3"),
            s.GraphIndexConfig("graph", scope, graph_vocabulary),
            sources,
        )
    )
    legs = [s.HybridLeg(name) for name in ["memory", "documents", "journal"]]
    if embedder is not None:
        legs.extend(
            s.HybridLeg(name, s.Semantic(embedder.space))
            for name in ["memory", "documents"]
        )
    search = s.SearchEngine(
        s.SearchConfig(
            scope,
            s.HybridPlan(legs),
            graph_expansion=None if graph is None else s.GraphExpansion("graph"),
        ),
        sources if graph is None else [*sources, graph],
    )
    agent = await finstack_ai.Agent.from_python(
        model,
        [
            memory.toolset(read=False),
            search.toolset(),
            documents.toolset(),
            finstack_ai.SkillsToolset("finstack.know.tools.skills"),
            finstack_ai.ElicitationToolset(ask_user=True),
        ],
        BASE_INSTRUCTION,
        output_type=output_type,
        capabilities=[CITATIONS_CAPABILITY],
        context_providers=[
            finstack_ai.RepositoryContextProvider(
                str(_SELF_DOCS_DIR), allowlist=_SELF_DOCS_ALLOWLIST
            ),
            search.context_provider(),
        ],
        middleware=[
            finstack_ai.InstructionsMiddleware([GROUNDING_POLICY]),
            finstack_ai.CompactionMiddleware.sliding_window(*COMPACTION_TOKENS),
        ],
        observers=[
            finstack_ai.LogObserver(str(data_dir / "events.ndjson")),
            memory.observer(),
            hints,
            *observers,
        ],
        sqlite_path=str(journal_path),
        sqlite_durability=finstack_ai.SqliteDurability.Durable,
        artifact_store=owner.artifact_store,
    )
    composition = KnowledgeAgent(
        agent,
        search,
        memory_source,
        documents,
        journal,
        graph,
        hints,
        authorized,
        embedder is not None,
    )
    composition.initial_maintenance = await composition.maintain(64)
    return composition


def scripted_model(
    responses: list[dict[str, Any] | str],
    *,
    component: str = "knowledge.model.scripted",
    index_attachment: bool = False,
) -> finstack_ai.PythonModel:
    """A ``PythonModel`` that replays canned responses in order.

    Each entry is either a final text (``str``) or a full response dict
    (for tool calls). Running past the script raises, which keeps notebook
    cells honest about how many model turns they expect. With index_attachment,
    the first response indexes the exact uploaded reference in the actual request.
    """

    queue: list[dict[str, Any] | str] = list(responses)
    needs_index = index_attachment

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        nonlocal needs_index
        del context
        if needs_index:
            reference = next(
                line.removeprefix("Document source reference: ")
                for message in request["messages"]
                for block in message["content"]
                if block["kind"] == "text"
                for line in block["text"].splitlines()
                if line.startswith("Document source reference: ")
            )
            needs_index = False
            return {
                "text": "",
                "completion_id": "index-upload",
                "tool_calls": [
                    {"name": "index_document", "arguments": json.loads(reference)}
                ],
            }
        if not queue:
            raise RuntimeError("scripted model exhausted")
        entry = queue.pop(0)
        if isinstance(entry, str):
            return {"text": entry, "completion_id": f"scripted-{len(queue)}"}
        return {"completion_id": f"scripted-{len(queue)}", **entry}

    return finstack_ai.PythonModel(
        callback,
        component=component,
        provider="knowledge-scripted",
        model="knowledge-scripted-model",
        context_window_tokens=131_072,
    )


def golden_entries() -> list[dict[str, Any]]:
    """Load the shared golden fixture (same file the Rust tests run)."""

    fixture = json.loads(_GOLDEN_PATH.read_text(encoding="utf-8"))
    entries = fixture["entries"]
    if not isinstance(entries, list) or not entries:
        raise ValueError("golden fixture has no entries")
    return entries
