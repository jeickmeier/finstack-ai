"""Shared knowledge-agent composition for the k-track notebooks.

Mirrors the Rust definition in ``apps/finstack-knowledge`` (spec §6,
option a: each surface composes the same agent in its own language; the
golden-questions fixture holds the surfaces together).

Divergences from the Rust composition: none. Every component the CLI
composes is the same native extension here — instructions middleware,
sliding-window compaction, the self-docs repository provider, the
document toolset + ingest middleware (auto-registered), memory
toolset/recall/capture, the native skills toolset with model-driven
capability activation, the log observer, and the durable artifact store.
Two deliberate surface choices remain (configuration, not gaps): the log
observer writes to ``<data_dir>/events.ndjson`` instead of the CLI's
stderr, and the self-docs are read from the app crate's ``docs/`` tree
instead of materialized under the data dir.
"""

from __future__ import annotations

import json
from collections.abc import Iterable
from pathlib import Path
from typing import Any

import finstack_ai

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
        "When answering from retrieved documents or memory, name the source "
        "(document name or memory record) for each claim and quote sparingly."
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


async def build_knowledge_agent(
    data_dir: Path,
    model: finstack_ai.PythonModel,
    *,
    tenant: str = "python-local",
    memory: finstack_ai.MemoryExtension | None = None,
    observers: Iterable[Any] = (),
) -> finstack_ai.Agent:
    """Compose the knowledge agent over ``data_dir`` (option a mirror).

    One sqlite journal at ``<data_dir>/journal.sqlite3`` and one durable
    artifact store at ``<data_dir>/artifacts`` (the same layout the CLI
    uses), the self-docs repository provider, the memory extension's
    toolset/recall-provider/observer, instructions + sliding-window
    compaction middleware, the native skills toolset (the citations
    capability is model-activatable through it), the elicitation toolset,
    and the NDJSON log observer.
    """

    data_dir.mkdir(parents=True, exist_ok=True)
    memory = memory or knowledge_memory(data_dir, tenant=tenant)
    return await finstack_ai.Agent.from_python(
        model,
        [
            memory.toolset(),
            finstack_ai.SkillsToolset("finstack.know.tools.skills"),
            finstack_ai.ElicitationToolset(ask_user=True),
        ],
        BASE_INSTRUCTION,
        capabilities=[CITATIONS_CAPABILITY],
        context_providers=[
            finstack_ai.RepositoryContextProvider(
                str(_SELF_DOCS_DIR), allowlist=_SELF_DOCS_ALLOWLIST
            ),
            memory.context_provider(max_hits=4),
        ],
        middleware=[
            finstack_ai.InstructionsMiddleware([GROUNDING_POLICY]),
            finstack_ai.CompactionMiddleware.sliding_window(*COMPACTION_TOKENS),
        ],
        observers=[
            finstack_ai.LogObserver(str(data_dir / "events.ndjson")),
            memory.observer(),
            *observers,
        ],
        sqlite_path=str(data_dir / "journal.sqlite3"),
        sqlite_durability=finstack_ai.SqliteDurability.Durable,
        artifact_path=str(data_dir / "artifacts"),
    )


def scripted_model(
    responses: list[dict[str, Any] | str],
    *,
    component: str = "knowledge.model.scripted",
) -> finstack_ai.PythonModel:
    """A ``PythonModel`` that replays canned responses in order.

    Each entry is either a final text (``str``) or a full response dict
    (for tool calls). Running past the script raises, which keeps notebook
    cells honest about how many model turns they expect.
    """

    queue: list[dict[str, Any] | str] = list(responses)

    async def callback(
        context: finstack_ai.CallbackContext, request: dict[str, Any]
    ) -> dict[str, Any]:
        del context, request
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
