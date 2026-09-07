"""Actual native retrieval, committed indexing effects and four-source Python parity."""

from __future__ import annotations

import asyncio
import inspect
import json
import sqlite3
from dataclasses import replace
from pathlib import Path
from typing import Any

import finstack_ai as ai
import pytest
from finstack_ai import search as s

SCOPE = s.SearchScope("python-local")


def model(callback: ai._finstack_ai.Callback) -> ai.PythonModel:
    return ai.PythonModel(
        callback,
        component="test.model.search",
        provider="offline",
        model="search-fixture",
        context_window_tokens=1_048_576,
    )


async def answer(_: ai.CallbackContext, __: dict[str, Any]) -> ai.ModelOutput:
    return {"text": "Gamma owns Delta", "completion_id": "final"}


async def remember(
    memory: ai.MemoryExtension,
    body: str = "Acme owns Beta",
    *,
    record_id: str = "ownership",
) -> None:
    calls = 0

    async def write(_: ai.CallbackContext, request: dict[str, Any]) -> ai.ModelOutput:
        nonlocal calls
        calls += 1
        if calls == 1:
            return {
                "text": "",
                "completion_id": "remember",
                "tool_calls": [
                    {
                        "name": "remember",
                        "arguments": {
                            "id": record_id,
                            "body": body,
                            "keywords": ["Acme", "Beta"],
                        },
                    }
                ],
            }
        assert '"is_error": true' not in json.dumps(request)
        return {"text": "stored", "completion_id": "stored"}

    agent = await ai.Agent.from_python(
        model(write), [memory.toolset()], document_tools=False
    )
    assert (await agent.run("Store the ownership fact")).text == "stored"


def vocabulary() -> s.GraphVocabulary:
    return s.GraphVocabulary(
        entity_kinds=["company"],
        edge_kinds=[s.EdgeKind("owns", "company", "company")],
        entity_rules=[s.EntityRule("company", r"(?P<label>Acme|Beta|Gamma|Delta)")],
        edge_rules=[
            s.EdgeRule(
                "owns",
                r"(?P<source>Acme|Beta|Gamma|Delta) owns (?P<target>Acme|Beta|Gamma|Delta)",
            )
        ],
    )


def test_four_sources_real_tool_indexing_hints_and_expansion(tmp_path: Path) -> None:
    async def exercise() -> None:
        memory = ai.MemoryExtension.sqlite(
            path=str(tmp_path / "memory.sqlite"), manage=True
        )
        await remember(memory)
        mem = s.MemorySearchSource(memory, s.MemorySearchConfig("memory", SCOPE))
        owner = await ai.Agent.from_python(
            model(answer),
            sqlite_path=str(tmp_path / "journal.sqlite"),
            artifact_path=str(tmp_path / "artifacts"),
        )
        session = await owner.create_session("python-local")
        journal_config = s.JournalIndexConfig("journal", SCOPE, [session.session_id])
        journal = s.JournalSearchSource(
            owner, str(tmp_path / "journal-index.sqlite"), journal_config
        )
        observer = journal.observer()
        writer = await ai.Agent.from_python(
            model(answer),
            observers=[observer],
            sqlite_path=str(tmp_path / "journal.sqlite"),
            artifact_store=owner.artifact_store,
            document_tools=False,
        )
        lane = await session.lane("main")
        assert (
            await lane.run(writer, "Record Gamma ownership").result()
        ).text == "Gamma owns Delta"
        assert (await journal.search(s.SearchQuery("Gamma")))[
            "historical_complete"
        ] is False
        reports = await observer.drain(1, 100_000)
        assert len(reports) == 1 and reports[0]["complete"]
        assert observer.dropped_hints == 0
        backfill = await journal.sync_session(session.session_id)
        assert backfill["indexed"] == 0 and backfill["historical_complete"]

        docs_config = s.DocumentIndexConfig("documents", SCOPE)
        docs = s.DocumentSearchSource(
            owner, str(tmp_path / "documents.sqlite"), docs_config
        )
        receipt: dict[str, Any] = {}
        calls = 0
        attachment: dict[str, Any] = {}

        async def ingest(
            _: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal calls, attachment
            calls += 1
            if calls <= 2:
                if calls == 1:
                    texts = [
                        b["text"]
                        for m in request["messages"]
                        for b in m["content"]
                        if b["kind"] == "text"
                    ]
                    line = next(
                        line
                        for text in texts
                        for line in text.splitlines()
                        if line.startswith("Document source reference: ")
                    )
                    attachment = json.loads(
                        line.removeprefix("Document source reference: ")
                    )
                return {
                    "text": "",
                    "completion_id": f"index-{calls}",
                    "tool_calls": [{"name": "index_document", "arguments": attachment}],
                }
            encoded = json.dumps(request)
            assert '"is_error": true' not in encoded
            assert '"chunks": 1' in encoded
            receipt.update(request)
            return {"text": "indexed", "completion_id": "indexed"}

        indexer = await ai.Agent.from_python(
            model(ingest),
            [docs.toolset()],
            sqlite_path=str(tmp_path / "journal.sqlite"),
            artifact_store=owner.artifact_store,
        )
        indexed = await indexer.run(
            "Index this document twice to check idempotency",
            attachments=[
                ai.Attachment(
                    "text/markdown",
                    data=b"# Ownership\n\nBeta owns Gamma",
                    name="ownership.md",
                )
            ],
        )
        assert indexed.text == "indexed" and receipt
        inputs = await docs.inputs()
        assert len(inputs) == 1
        first = await docs.index_document(inputs[0][1])
        assert first["chunks"] == 1
        assert await docs.indexed_document(inputs[0][1]) == first
        assert first == await docs.index_document(inputs[0][1])
        reopened = s.DocumentSearchSource(
            owner, str(tmp_path / "documents.sqlite"), docs_config
        )
        assert await docs.search(s.SearchQuery("Beta")) == await reopened.search(
            s.SearchQuery("Beta")
        )
        rebuilt = s.DocumentSearchSource(
            owner, str(tmp_path / "rebuilt.sqlite"), docs_config
        )
        assert await rebuilt.index_document(inputs[0][1]) == first
        assert await rebuilt.search(s.SearchQuery("Beta")) == await docs.search(
            s.SearchQuery("Beta")
        )

        sources = [mem, docs, journal]
        graph = s.GraphSearchSource(
            str(tmp_path / "graph.sqlite"),
            s.GraphIndexConfig("graph", SCOPE, vocabulary()),
            sources,
        )
        for source, term in zip(sources, ["Acme", "Beta", "Gamma"], strict=True):
            result = await source.search(s.SearchQuery(term))
            assert result["hits"]
            for hit in result["hits"]:
                assert hit["provenance"]["scope"]["tenant"] == "python-local"
                assert (await graph.index_reference(hit["source"], hit["reference"]))[
                    "entities"
                ] >= 1
        rebuilt_graph = s.GraphSearchSource(
            str(tmp_path / "rebuilt-graph.sqlite"),
            s.GraphIndexConfig("graph", SCOPE, vocabulary()),
            sources,
        )
        cursor = None
        indexed_count = 0
        for _ in range(256):
            page = await rebuilt_graph.rebuild_step(cursor=cursor, limit=1)
            assert page["indexed"] <= 1 and page["unavailable"] == 0
            indexed_count += page["indexed"]
            cursor = page["next_cursor"]
            if cursor is None:
                break
        assert cursor is None and indexed_count >= 3
        assert await rebuilt_graph.search(
            s.SearchQuery("Acme", s.Graph("entity"))
        ) == await graph.search(s.SearchQuery("Acme", s.Graph("entity")))
        for source in sources:
            catalog = await source.references(limit=1)
            assert len(catalog["references"]) <= 1
            for reference in catalog["references"]:
                assert await source.read_evidence(reference) is not None

        plan = s.HybridPlan(
            [s.HybridLeg("memory"), s.HybridLeg("documents"), s.HybridLeg("journal")]
        )
        config = s.SearchConfig(SCOPE, plan, graph_expansion=s.GraphExpansion("graph"))
        engine = s.SearchEngine(config, [*sources, graph])
        response = await engine.search(s.SearchRequest("Acme", limit=32))
        assert {h["source"] for h in response["hits"]} == {
            "memory",
            "documents",
            "journal",
            "graph",
        }
        assert {o["source"] for o in response["outcomes"]} == {
            "memory",
            "documents",
            "journal",
            "graph",
        }
        assert all(h["evidence"] for h in response["hits"])
        no_expansion = s.SearchEngine(
            replace(config, graph_expansion=None), [*sources, graph]
        )
        assert {
            h["source"]
            for h in (await no_expansion.search(s.SearchRequest("Acme")))["hits"]
        } == {"memory"}
        path = await graph.search(
            s.SearchQuery("Acme", s.Graph("path", target="Delta", depth=3))
        )
        assert path["hits"] and any(h["entity_label"] == "delta" for h in path["hits"])
        assert all(
            c["scope"]["tenant"] == "python-local"
            for h in path["hits"]
            for c in h["provenance"]["citations"]
        )
        assert any(
            h["reference"]["first_record"]
            for h in (await journal.search(s.SearchQuery("Gamma")))["hits"]
        )

        # The same tool is actually dispatched through the SDK, with global recall.
        search_calls = 0
        search_requests = []

        async def query(
            _: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal search_calls
            search_calls += 1
            search_requests.append(request)
            if search_calls == 1:
                return {
                    "text": "",
                    "completion_id": "query",
                    "tool_calls": [
                        {
                            "name": "search",
                            "arguments": {
                                "text": "Acme",
                                "limit": 32,
                                "strategy": None,
                                "sources": None,
                            },
                        }
                    ],
                }
            return {"text": "retrieved", "completion_id": "retrieved"}

        reader = await ai.Agent.from_python(
            model(query),
            [engine.toolset()],
            context_providers=[engine.context_provider()],
            document_tools=False,
        )
        assert (await reader.run("Acme")).text == "retrieved"
        assert search_calls == 2
        assert "search_coverage" in json.dumps(search_requests[0])
        encoded = json.dumps(search_requests[-1]).replace('\\"', '"')
        assert '"is_error": true' not in encoded
        for name in ["memory", "documents", "journal", "graph"]:
            assert f'"source":"{name}"' in encoded.replace(" ", "")

    asyncio.run(exercise())


def test_scope_errors_bounds_and_failure_outcomes(tmp_path: Path) -> None:
    async def exercise() -> None:
        memory = ai.MemoryExtension.in_process(tenant="t", user="u")
        for scope in [s.SearchScope("other", user="u"), s.SearchScope("t")]:
            with pytest.raises(ai.RuntimeError) as failed:
                s.MemorySearchSource(memory, s.MemorySearchConfig("memory", scope))
            assert failed.value.code == "search_scope_denied"
        with pytest.raises(ai.RuntimeError) as failed:
            s.MemorySearchSource(
                ai.MemoryExtension.in_process(read=False),
                s.MemorySearchConfig("memory", SCOPE),
            )
        assert failed.value.code == "search_scope_denied"
        mem = s.MemorySearchSource(
            memory,
            s.MemorySearchConfig(
                "memory",
                s.SearchScope("t", user="u"),
                scope_mapping="restrict_to_bound",
            ),
        )
        engine = s.SearchEngine(
            s.SearchConfig(s.SearchScope("t"), s.HybridPlan([s.HybridLeg("memory")])),
            [mem],
        )
        with pytest.raises(ai.RuntimeError) as failed:
            await engine.search(s.SearchRequest("x", strategy=s.Lexical("literal")))
        assert failed.value.code == "search_no_successful_sources"
        assert failed.value.context["outcomes"][0]["status"] == "unsupported"
        with pytest.raises(ai.RuntimeError):
            await engine.search(s.SearchRequest("x", limit=257))
        with pytest.raises(ai.RuntimeError):
            await engine.search(
                s.SearchRequest("x", strategy=s.Graph("neighborhood", depth=9))
            )
        owner = await ai.Agent.from_python(model(answer), document_tools=False)
        docs = s.DocumentSearchSource(
            owner, str(tmp_path / "docs.sqlite"), s.DocumentIndexConfig("docs", SCOPE)
        )
        with pytest.raises(ai.RuntimeError) as failed:
            await ai.Agent.from_python(
                model(answer), [docs.toolset()], document_tools=False
            )
        assert failed.value.code == "search_scope_denied"
        session = await owner.create_session("python-local")
        journal = s.JournalSearchSource(
            owner,
            str(tmp_path / "journal.sqlite"),
            s.JournalIndexConfig("journal", SCOPE, [session.session_id]),
        )
        with pytest.raises(ai.RuntimeError) as failed:
            await journal.sync_session("00000000-0000-0000-0000-000000000001")
        assert failed.value.code == "search_scope_denied"
        foreign = tmp_path / "foreign.sqlite"
        with sqlite3.connect(foreign) as db:
            db.execute("PRAGMA user_version=999")
        with pytest.raises(ai.RuntimeError):
            s.DocumentSearchSource(
                owner, str(foreign), s.DocumentIndexConfig("foreign", SCOPE)
            )

    asyncio.run(exercise())


def test_exact_embeddings_and_deterministic_fusion(tmp_path: Path) -> None:
    async def exercise() -> None:
        embedder = s.HashEmbedder(64)
        memory = ai.MemoryExtension.sqlite(path=str(tmp_path / "memory.sqlite"))
        await remember(memory)
        mem = s.MemorySearchSource(
            memory, s.MemorySearchConfig("memory", SCOPE), embedder=embedder
        )
        semantic = s.SearchQuery("Acme owns Beta", s.Semantic(embedder.space))
        assert (await mem.search(semantic))["status"] == "unavailable"
        progress = await mem.reconcile_embeddings(limit=1)
        assert progress == {"examined": 1, "total": 1, "next_offset": None}
        assert (await mem.search(semantic))["status"] == "completed"
        assert (await mem.search(semantic))["hits"][0]["reference"] == {
            "kind": "memory",
            "id": "ownership",
        }
        for fusion, expected in [
            (s.Rrf(), 16_393_442),
            (s.WeightedSum(), 1_000_000_000),
        ]:
            engine = s.SearchEngine(
                s.SearchConfig(SCOPE, s.HybridPlan([s.HybridLeg("memory")], fusion)),
                [mem],
            )
            result = await engine.search(s.SearchRequest("Acme"))
            assert len(result["hits"]) == 1
            hit = result["hits"][0]
            assert hit["score"] == expected
            assert hit["evidence"][0]["contribution"] == expected
            assert hit["evidence"][0]["rank"] == 1
        hybrid = s.SearchEngine(
            s.SearchConfig(
                SCOPE,
                s.HybridPlan(
                    [
                        s.HybridLeg("memory"),
                        s.HybridLeg("memory", s.Semantic(embedder.space)),
                    ]
                ),
            ),
            [mem],
        )
        hit = (await hybrid.search(s.SearchRequest("Acme owns Beta")))["hits"][0]
        assert len(hit["evidence"]) == 2
        assert hit["score"] == 2 * 16_393_442

    asyncio.run(exercise())


def test_public_search_typing_and_hover() -> None:
    for name in s.__all__:
        assert hasattr(s, name), name
    for value in [
        s.SearchEngine,
        s.SearchEngine.search,
        s.SearchSource.search,
        s.DocumentSearchSource,
        s.GraphSearchSource,
        s.SearchScope,
        s.SearchResponse,
        ai.ArtifactStore,
    ]:
        assert inspect.getdoc(value), value
    assert list(inspect.signature(s.SearchEngine.search).parameters) == [
        "self",
        "request",
    ]
    assert "artifact_store" in inspect.signature(ai.Agent.from_python).parameters
