"""Offline knowledge composition: real effects, maintained catalogs and disk reopen."""

from __future__ import annotations

import asyncio
import json
from pathlib import Path
from typing import Any

import finstack_ai as ai
from _knowledge import build_knowledge_agent, scripted_model
from finstack_ai import search as s


def test_knowledge_maintains_four_sources(tmp_path: Path) -> None:
    async def exercise() -> None:
        owner = await build_knowledge_agent(tmp_path, scripted_model([]))
        session = await owner.agent.create_session("python-local")
        vocabulary = s.GraphVocabulary(
            ["company"],
            [s.EdgeKind("owns", "company", "company")],
            [s.EntityRule("company", r"(?P<label>Acme|Beta|Gamma|Delta)")],
            [
                s.EdgeRule(
                    "owns",
                    r"(?P<source>Acme|Beta|Gamma|Delta) owns (?P<target>Acme|Beta|Gamma|Delta)",
                )
            ],
        )
        calls = 0

        async def ingest(
            _: ai.CallbackContext, request: dict[str, Any]
        ) -> ai.ModelOutput:
            nonlocal calls
            calls += 1
            if calls == 1:
                lines = [
                    line
                    for message in request["messages"]
                    for block in message["content"]
                    if block["kind"] == "text"
                    for line in block["text"].splitlines()
                ]
                source = next(
                    line.removeprefix("Document source reference: ")
                    for line in lines
                    if line.startswith("Document source reference: ")
                )
                return {
                    "text": "",
                    "completion_id": "index",
                    "tool_calls": [
                        {"name": "index_document", "arguments": json.loads(source)}
                    ],
                }
            assert '"is_error": true' not in json.dumps(request)
            if calls == 2:
                return {
                    "text": "",
                    "completion_id": "remember",
                    "tool_calls": [
                        {
                            "name": "remember",
                            "arguments": {
                                "body": "Acme owns Beta",
                                "keywords": ["Acme", "owns"],
                            },
                        }
                    ],
                }
            return {"text": "Gamma owns Delta", "completion_id": "final"}

        model = ai.PythonModel(
            ingest,
            component="knowledge.model.index",
            provider="offline",
            model="offline",
            context_window_tokens=131_072,
        )
        embedder = s.HashEmbedder(16)
        app = await build_knowledge_agent(
            tmp_path, model, graph_vocabulary=vocabulary, embedder=embedder
        )
        assert app.sessions == (session.session_id,)
        lane = await session.lane("main")
        assert (
            await lane.run(
                app.agent,
                "Index ownership",
                attachments=[
                    ai.Attachment(
                        "text/markdown",
                        data=b"# Ownership\n\nBeta owns Gamma",
                        name="ownership.md",
                    )
                ],
            ).result()
        ).text == "Gamma owns Delta"
        for _ in range(32):
            report = await app.maintain(256)
            assert not report["failures"]
            if not report["more"]:
                break
        assert not report["more"]
        result = await app.search.search(s.SearchRequest("Acme", limit=32))
        assert {hit["source"] for hit in result["hits"]} == {
            "memory",
            "documents",
            "journal",
            "graph",
        }
        assert all(hit["evidence"] for hit in result["hits"])
        assert (
            await app.memory.search(s.SearchQuery("Acme", s.Semantic(embedder.space)))
        )["status"] == "completed"
        # Fresh source handles use the application's own persisted session catalog.
        reopened = await build_knowledge_agent(
            tmp_path, scripted_model([]), graph_vocabulary=vocabulary
        )
        assert reopened.sessions == (session.session_id,)
        again = await reopened.search.search(s.SearchRequest("Acme", limit=32))
        assert {hit["source"] for hit in again["hits"]} == {
            "memory",
            "documents",
            "journal",
            "graph",
        }

    asyncio.run(exercise())
