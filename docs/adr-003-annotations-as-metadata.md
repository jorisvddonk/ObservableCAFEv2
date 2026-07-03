# ADR-003: Annotations as metadata

**Status**: Accepted

**Context**: Chunks carry text, binary, or null content. But chunks also need metadata: chat role, model name, security labels, tool calls, RPC messages, config overrides. A rigid typed schema for every metadata variant would create tight coupling between producers and consumers.

**Decision**: Every chunk carries `annotations: HashMap<String, serde_json::Value>` — an open map of key/value metadata. Producers add whatever annotations they need. Consumers read what they understand and ignore the rest. Standard keys are documented in `cafe-types/src/annotation.rs` (`chat.role`, `jsonrpc.request`, `transient`, etc.) but any key is valid.

**Consequences**:
- Zero schema coupling — TTS can add `tts.profile` without coordinating with other services
- Extensible without code changes — new annotation keys just start working
- Forward-compatible — old consumers ignore unknown keys
- No type safety — typos in annotation keys produce silent failures
- Discovery requires documentation (keys are strings, not enums)
