from __future__ import annotations

import json
import os
from typing import AsyncIterator, Iterator, Optional


class CafeClient:
    def __init__(self, base_url: str = "http://localhost:4000", token: Optional[str] = None):
        self.base_url = base_url.rstrip("/")
        self.token = token or os.environ.get("CAFE_TOKEN") or ""
        self._headers = {"Content-Type": "application/json"}
        if self.token:
            self._headers["Authorization"] = f"Bearer {self.token}"

    # -- sync helpers -------------------------------------------------------

    def _sync_post(self, path: str, json_body: dict) -> dict:
        import httpx
        r = httpx.post(
            f"{self.base_url}{path}",
            json=json_body,
            headers=self._headers,
            timeout=30,
        )
        r.raise_for_status()
        if r.text:
            return r.json()
        return {}

    def _sync_delete(self, path: str) -> None:
        import httpx
        httpx.delete(f"{self.base_url}{path}", headers=self._headers, timeout=10)

    def _sync_get(self, path: str) -> dict:
        import httpx
        r = httpx.get(f"{self.base_url}{path}", headers=self._headers, timeout=10)
        r.raise_for_status()
        return r.json()

    # -- async helpers ------------------------------------------------------

    async def _async_post(self, path: str, json_body: dict) -> dict:
        import httpx
        async with httpx.AsyncClient() as c:
            r = await c.post(
                f"{self.base_url}{path}",
                json=json_body,
                headers=self._headers,
                timeout=30,
            )
            r.raise_for_status()
            if r.text:
                return r.json()
            return {}

    async def _async_delete(self, path: str) -> None:
        import httpx
        async with httpx.AsyncClient() as c:
            await c.delete(f"{self.base_url}{path}", headers=self._headers, timeout=10)

    # -- session management -------------------------------------------------

    def create_session_sync(self, agent_id: str, system_prompt: Optional[str] = None) -> str:
        body = {"agent_id": agent_id, "config": {}}
        if system_prompt:
            body["config"]["system_prompt"] = system_prompt
        result = self._sync_post("/api/sessions?token=" + self.token, body)
        return result["id"]

    async def create_session_async(self, agent_id: str, system_prompt: Optional[str] = None) -> str:
        body = {"agent_id": agent_id, "config": {}}
        if system_prompt:
            body["config"]["system_prompt"] = system_prompt
        result = await self._async_post("/api/sessions?token=" + self.token, body)
        return result["id"]

    def delete_session_sync(self, session_id: str) -> None:
        self._sync_delete(f"/api/sessions/{session_id}?token={self.token}")

    async def delete_session_async(self, session_id: str) -> None:
        await self._async_delete(f"/api/sessions/{session_id}?token={self.token}")

    # -- chat ---------------------------------------------------------------

    def chat_sync(self, session_id: str, content: str) -> Iterator[str]:
        import httpx
        with httpx.stream(
            "POST",
            f"{self.base_url}/api/sessions/{session_id}/chat?token={self.token}",
            json={"content": content},
            headers=self._headers,
            timeout=120,
        ) as r:
            r.raise_for_status()
            buf = ""
            for chunk in r.iter_bytes():
                buf += chunk.decode("utf-8")
                while "\n" in buf:
                    line, buf = buf.split("\n", 1)
                    line = line.strip()
                    if not line or not line.startswith("data: "):
                        continue
                    payload = line[6:]
                    if payload.startswith("{"):
                        ev = json.loads(payload)
                        if ev.get("content_type") == "text" and ev.get("content"):
                            yield ev["content"]
                        annotations = ev.get("annotations") or {}
                        if annotations.get("chat.stream_complete"):
                            return
                        if ev.get("content_type") == "null" and annotations.get("chat.stream_complete"):
                            return

    async def chat_async(self, session_id: str, content: str) -> AsyncIterator[str]:
        import httpx
        async with httpx.AsyncClient() as c:
            async with c.stream(
                "POST",
                f"{self.base_url}/api/sessions/{session_id}/chat?token={self.token}",
                json={"content": content},
                headers=self._headers,
                timeout=120,
            ) as r:
                r.raise_for_status()
                buf = ""
                async for chunk in r.aiter_bytes():
                    buf += chunk.decode("utf-8")
                    while "\n" in buf:
                        line, buf = buf.split("\n", 1)
                        line = line.strip()
                        if not line or not line.startswith("data: "):
                            continue
                        payload = line[6:]
                        if payload.startswith("{"):
                            ev = json.loads(payload)
                            if ev.get("content_type") == "text" and ev.get("content"):
                                yield ev["content"]
                            annotations = ev.get("annotations") or {}
                            if annotations.get("chat.stream_complete"):
                                return
                            if ev.get("content_type") == "null" and annotations.get("chat.stream_complete"):
                                return
