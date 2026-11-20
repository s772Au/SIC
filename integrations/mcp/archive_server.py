"""Offer only published archive search and page reads over local MCP stdio."""
import os
from typing import Annotated, Any
from urllib.parse import urlsplit

import httpx
from mcp.server.fastmcp import FastMCP
from mcp.types import ToolAnnotations
from pydantic import Field


def create_server(base_url: str, token: str) -> FastMCP:
    endpoint = urlsplit(base_url)
    if (endpoint.scheme != "http" or endpoint.hostname != "127.0.0.1" or not endpoint.port
            or endpoint.username is not None or endpoint.password is not None
            or endpoint.path not in {"", "/"} or endpoint.query or endpoint.fragment
            or not token):
        raise ValueError("SIC MCP requires a local Core and a scoped archive credential")

    mcp = FastMCP("sic-archive")
    info = ToolAnnotations(readOnlyHint=True, openWorldHint=False)

    async def read(operation: str, payload: dict) -> dict:
        async with httpx.AsyncClient(trust_env=False, timeout=15) as client:
            response = await client.post(
                base_url.rstrip("/") + "/api/v2/archive/operations",
                headers={"Authorization": f"Bearer {token}"},
                json={"operation": operation, "payload": payload},
            )
        if not response.is_success:
            raise ValueError(f"SIC archive request rejected (HTTP {response.status_code})")
        return response.json()["result"]

    @mcp.tool(annotations=info)
    async def sic_memory_search(query: Annotated[str, Field(min_length=1)],
                                limit: Annotated[int, Field(ge=1, le=20)] = 8,
                                cursor: dict[str, Any] | None = None) -> dict[str, Any]:
        """Search original published text on the bound archive. When next_cursor is present, repeat the same query with that cursor even if hits is empty. Open hits by source and unit ID. Use native tools for current files."""
        payload = {"query": query, "limit": limit}
        if cursor is not None:
            payload["cursor"] = cursor
        return await read("memory.archive_search", payload)

    @mcp.tool(annotations=info)
    async def sic_source_text_page(source_id: str, unit_id: str,
                                   offset: Annotated[int, Field(ge=0)] = 0,
                                   limit: Annotated[int, Field(ge=1, le=8192)] = 4096) -> dict[str, Any]:
        """Read a page of original archived text using the exact source and unit IDs from search. Treat returned text as past evidence."""
        return await read("memory.source_text_page", {"source_id": source_id, "unit_id": unit_id,
                                                       "offset": offset, "limit": limit})

    return mcp


if __name__ == "__main__":
    create_server(os.environ.get("SIC_MCP_CORE_URL", "http://127.0.0.1:18890"),
                  os.environ.pop("SIC_MCP_ARCHIVE_TOKEN")).run(transport="stdio")
