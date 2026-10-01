# ADR-136: Structured LLM model catalog in the registry session

**Status**: Implemented

**Date**: 2026-10-01

**Driver**: [ADR-134](./adr-134-per-session-llm-backend.md) made the LLM backend
selectable per session via `config.llm.backend`, but there was no way to know
*which backend serves which model*. cafe-llm's `_cafe_llm_registry` session
published only a flat `config.available_models` list of names, and the
process default is not necessarily the provider a given model belongs to (a
local proxy and OpenCode Go can both advertise overlapping or unrelated model
names). The web's `/model` picker needs to set the model **and** the correct
backend together.

**Context**: The catalog is produced by cafe-llm (which owns the router and
therefore the backend→model mapping), must be readable by any bus client, and
should not require a new persistence mechanism. The registry session already
exists for exactly this kind of runtime metadata.

**Decision**:

1. **A shared type, not ad-hoc annotations.**
   `cafe_types::ModelCatalog { default_backend, backends: Vec<BackendModels> }`
   with `BackendModels { backend, default_model, models }`, plus helpers
   `all_models()` and `backend_for(model, preferred)`. It is serialized into a
   single `config.model_catalog` annotation (new `keys::CONFIG_MODEL_CATALOG`).
2. **cafe-llm publishes it.** `BackendRouter` exposes `list_backends()` and
   `default_backend()`; `publish_model_registry` writes the catalog (and keeps
   `config.available_models` as a flat compatibility list) on the
   `_cafe_llm_registry` session.
3. **The web reads the session, not a bespoke feed.** `readModelCatalog()` in
   `cafe-web-sdk` fetches the registry session's history via the existing
   `getHistory` API and parses the catalog (`parseModelCatalog`), falling back
   to the flat list. `GET /api/models` also returns the same catalog for HTTP
   consumers, but the UI deliberately reads the session directly.
4. **Selection uses the catalog.** `/model` resolves the served backend with
   `backendForModel(catalog, model, default_backend)` and publishes both
   `config.llm.model` and `config.llm.backend`.

**Consequences**:

- Picking a model routes correctly even when it is not served by the process
  default backend.
- The catalog is published per backend on a 60s refresh, so it reflects live
  provider state (models added/removed) without a restart.
- The backend→model mapping is derived from each backend's `list_models()`. For
  a proxy that advertises non-LLM assets (embeddings, TTS, ASR), those names
  appear under that backend; the catalog does not distinguish model kinds.
- `config.model_catalog` supersedes the flat list for new consumers; the flat
  key remains for backward compatibility.
- Adding a backend requires no catalog code — the router reports it.

**Alternatives considered**:

- *Infer the backend from the model name in the UI.* Rejected: model names
  overlap and carry no reliable backend prefix.
- *A dedicated `/api/models` feed only.* Rejected per request: the catalog is
  already a published session fact, and reading it directly keeps the UI in
  sync with exactly what cafe-llm published. The HTTP endpoint is retained for
  non-UI consumers.
- *Flat key plus a parallel backend list.* Rejected: two structures to keep in
  sync; a single typed catalog is simpler.

Related: [ADR-134](./adr-134-per-session-llm-backend.md),
[ADR-133](./adr-133-opencode-go-backend.md),
[ADR-121](./adr-121-evaluator-schema-system.md).
