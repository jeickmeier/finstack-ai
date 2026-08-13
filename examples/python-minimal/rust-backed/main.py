"""Minimal Rust-backed finstack-ai Python starter."""

from __future__ import annotations

import argparse
import asyncio

import finstack_ai


async def main(*, run: bool) -> None:
    """Construct the Rust-backed agent and optionally execute one request."""
    agent = await finstack_ai.Agent.openai_compatible(
        "http://127.0.0.1:8000/v1",
        "local-model",
        "Answer concisely.",
        capabilities=[
            finstack_ai.Capability(
                "starter.capability.research",
                "Research financial statements",
                ["Cite the supplied financial statement evidence."],
                activation="model",
            )
        ],
    )
    if not run:
        print(agent.compact_capability_catalog())
        return
    result = await agent.run("Summarize the supplied evidence.")
    print(result.text)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--run",
        action="store_true",
        help="send one request to the explicitly configured local endpoint",
    )
    args = parser.parse_args()
    asyncio.run(main(run=args.run))
