from __future__ import annotations

from typing import AsyncGenerator, Dict, Iterator, Optional

from llm import AsyncModel, AsyncResponse, Conversation, Model, Prompt, Response

from .client import CafeClient


class CafeModel(Model):
    can_stream = True
    needs_key = None

    def __init__(self, model_id: str, base_url: str = "http://localhost:4000", agent_id: str = "default"):
        self.model_id = model_id
        self.base_url = base_url
        self.agent_id = agent_id
        self._sessions: Dict[str, str] = {}

    def execute(
        self,
        prompt: Prompt,
        stream: bool,
        response: Response,
        conversation: Optional[Conversation],
    ) -> Iterator[str]:
        client = CafeClient(self.base_url)

        system = prompt.system or None

        if conversation is not None:
            session_id = self._sessions.get(conversation.id)
            if session_id is None:
                session_id = client.create_session_sync(self.agent_id, system_prompt=system)
                self._sessions[conversation.id] = session_id
            else:
                session_id = self._sessions[conversation.id]
        else:
            session_id = client.create_session_sync(self.agent_id, system_prompt=system)

        content = prompt.prompt or ""
        yield from client.chat_sync(session_id, content)

        if conversation is None:
            client.delete_session_sync(session_id)


class CafeAsyncModel(AsyncModel):
    can_stream = True
    needs_key = None

    def __init__(self, model_id: str, base_url: str = "http://localhost:4000", agent_id: str = "default"):
        self.model_id = model_id
        self.base_url = base_url
        self.agent_id = agent_id
        self._sessions: Dict[str, str] = {}

    async def execute(
        self,
        prompt: Prompt,
        stream: bool,
        response: AsyncResponse,
        conversation: Optional[Conversation],
    ) -> AsyncGenerator[str, None]:
        client = CafeClient(self.base_url)

        system = prompt.system or None

        if conversation is not None:
            session_id = self._sessions.get(conversation.id)
            if session_id is None:
                session_id = await client.create_session_async(self.agent_id, system_prompt=system)
                self._sessions[conversation.id] = session_id
            else:
                session_id = self._sessions[conversation.id]
        else:
            session_id = await client.create_session_async(self.agent_id, system_prompt=system)

        content = prompt.prompt or ""

        async for chunk in client.chat_async(session_id, content):
            yield chunk

        if conversation is None:
            await client.delete_session_async(session_id)
