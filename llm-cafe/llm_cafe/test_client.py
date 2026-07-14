from __future__ import annotations

import importlib.util
import json
import os
import asyncio
import sys
import types
import unittest
from typing import AsyncIterator, Iterator, List


def _make_sse(content: str, complete: bool = True) -> bytes:
    ev = {"content_type": "text", "content": content}
    if complete:
        ev["annotations"] = {"chat.stream_complete": True}
    line = "data: " + json.dumps(ev, ensure_ascii=False)
    return (line + "\n").encode("utf-8")


def _split_at_multibyte(data: bytes, marker: bytes) -> List[bytes]:
    idx = data.index(marker)
    # cut between the first and second byte of the multibyte sequence
    return [data[: idx + 1], data[idx + 1 :]]


class _FakeSyncResponse:
    def __init__(self, chunks: List[bytes]):
        self._chunks = chunks

    def raise_for_status(self) -> None:
        return None

    def iter_bytes(self) -> Iterator[bytes]:
        for c in self._chunks:
            yield c


class _FakeSyncStream:
    def __init__(self, chunks: List[bytes]):
        self._chunks = chunks

    def __enter__(self) -> _FakeSyncResponse:
        return _FakeSyncResponse(self._chunks)

    def __exit__(self, *exc) -> None:
        return None


class _FakeAsyncResponse:
    def __init__(self, chunks: List[bytes]):
        self._chunks = chunks

    def raise_for_status(self) -> None:
        return None

    async def aiter_bytes(self) -> AsyncIterator[bytes]:
        for c in self._chunks:
            yield c


class _FakeAsyncStream:
    def __init__(self, chunks: List[bytes]):
        self._chunks = chunks

    async def __aenter__(self) -> _FakeAsyncResponse:
        return _FakeAsyncResponse(self._chunks)

    async def __aexit__(self, *exc) -> None:
        return None


class _FakeAsyncClient:
    def __init__(self, chunks: List[bytes]):
        self._chunks = chunks

    async def __aenter__(self):
        return self

    async def __aexit__(self, *exc):
        return None

    def stream(self, *args, **kwargs):
        return _FakeAsyncStream(self._chunks)


def _install_fake_httpx(sync_chunks: List[bytes], async_chunks: List[bytes]) -> None:
    fake = types.ModuleType("httpx")
    fake.stream = lambda *a, **k: _FakeSyncStream(sync_chunks)

    class _AsyncClient:
        def __init__(self, *a, **k):
            pass

        async def __aenter__(self):
            return _FakeAsyncClient(async_chunks)

        async def __aexit__(self, *exc):
            return None

    fake.AsyncClient = _AsyncClient
    sys.modules["httpx"] = fake


def _load_client():
    here = os.path.dirname(os.path.abspath(__file__))
    spec = importlib.util.spec_from_file_location("_cafe_client", os.path.join(here, "client.py"))
    mod = importlib.util.module_from_spec(spec)
    sys.modules["_cafe_client"] = mod
    spec.loader.exec_module(mod)
    return mod


def _remove_fake_httpx() -> None:
    sys.modules.pop("httpx", None)


class TestSplitMultibyteChunk(unittest.TestCase):
    def test_chat_sync_split_euro(self):
        _install_fake_httpx(_split_at_multibyte(_make_sse("a€b"), b"\xe2"), [])
        try:
            client_mod = _load_client()
            client = client_mod.CafeClient(token="t")
            out = list(client.chat_sync("s", "hi"))
        finally:
            _remove_fake_httpx()
        self.assertEqual(out, ["a€b"])

    def test_chat_async_split_emoji(self):
        _install_fake_httpx([], _split_at_multibyte(_make_sse("x😀y"), b"\xf0\x9f"))
        try:
            client_mod = _load_client()
            client = client_mod.CafeClient(token="t")

            async def run():
                return [c async for c in client.chat_async("s", "hi")]

            out = asyncio.run(run())
        finally:
            _remove_fake_httpx()
        self.assertEqual(out, ["x😀y"])


if __name__ == "__main__":
    unittest.main()
