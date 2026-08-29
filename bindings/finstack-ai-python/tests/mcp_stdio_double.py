"""Scripted MCP stdio server double for `test_mcp_toolset.py`.

Speaks newline-delimited JSON-RPC: answers `tools/list` with one `echo`
tool, `prompts/list` with -32601 (deliberately unimplemented — the classify
layer tolerates it and the stdio transport must not poison on a clean
error response), other list catalogs with empty results, and `tools/call`
by echoing the `text` argument back as a text content block.
"""

from __future__ import annotations

import json
import sys


def _reply(payload: dict[str, object]) -> None:
    sys.stdout.write(json.dumps(payload) + "\n")
    sys.stdout.flush()


def main() -> None:
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        request = json.loads(line)
        request_id = request.get("id")
        method = request.get("method")
        if request_id is None:
            continue  # notification
        if method == "tools/list":
            _reply(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "tools": [
                            {
                                "name": "echo",
                                "title": "Echo",
                                "description": "Echo the text argument back.",
                                "inputSchema": {
                                    "type": "object",
                                    "properties": {"text": {"type": "string"}},
                                    "required": ["text"],
                                    "additionalProperties": False,
                                },
                            }
                        ]
                    },
                }
            )
        elif method == "tools/call":
            params = request.get("params") or {}
            arguments = params.get("arguments") or {}
            _reply(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "result": {
                        "content": [
                            {
                                "type": "text",
                                "text": f"echo: {arguments.get('text', '')}",
                            }
                        ]
                    },
                }
            )
        elif method == "prompts/list":
            _reply(
                {
                    "jsonrpc": "2.0",
                    "id": request_id,
                    "error": {"code": -32601, "message": "Method not found"},
                }
            )
        else:
            _reply({"jsonrpc": "2.0", "id": request_id, "result": {}})


if __name__ == "__main__":
    main()
