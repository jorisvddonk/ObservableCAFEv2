from llm import hookimpl
from .model import CafeModel, CafeAsyncModel


@hookimpl
def register_models(register, model_aliases):
    import os

    base_url = os.environ.get("CAFE_SERVER_URL", "http://localhost:4000")

    sync = CafeModel("cafe-default", base_url=base_url)
    async_ = CafeAsyncModel("cafe-default", base_url=base_url)
    register(sync, async_model=async_, aliases=["cafe"])

    agents = _list_agents(base_url)
    for agent in agents:
        aid = agent["id"]
        sync = CafeModel(f"cafe-{aid}", base_url=base_url, agent_id=aid)
        async_ = CafeAsyncModel(f"cafe-{aid}", base_url=base_url, agent_id=aid)
        register(sync, async_model=async_)


def _list_agents(base_url):
    import os
    try:
        import httpx

        token = os.environ.get("CAFE_TOKEN") or ""
        headers = {}
        if token:
            headers["Authorization"] = f"Bearer {token}"
        r = httpx.get(f"{base_url}/api/agents", headers=headers, timeout=3)
        r.raise_for_status()
        return r.json()
    except Exception:
        return []
