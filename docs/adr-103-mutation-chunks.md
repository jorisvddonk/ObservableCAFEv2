# ADR-103: Mutation chunks

**Status**: Implemented (`e4d5a9d`)

**Context**: Credentials, security labels, and other annotations need to be added to chunks after they're published. The event-sourced model (ADR-002) prohibits modifying published chunks. A separate "update" mechanism would fight the append-only model.

**Decision**: A chunk with `mutates.target_id: "<chunk-id>"` is a **mutation**. It overlays its annotations onto the target chunk. The merge is exclusively client-side — the original chunk in bus history is never modified. The merge is shallow: mutation annotation keys are inserted/overwritten onto the target's annotations. The `mutates.target_id` key itself is excluded from the merge.

Both write credentials (delivered via `direct_to`) and read credentials (broadcast) use the same mutation mechanism. The merge code is identical for both paths.

**Consequences**:
- No bus changes — mutations are regular chunks with a specific annotation
- Credentials can be attached to BinaryRef chunks without modifying them
- The mutation merge code is shared across cafe-tui and cafe-web
- Mutations can target chunks that haven't arrived yet (wire reordering) — held until target arrives
- Mutations follow the same persistence rules (`transient` flag)
