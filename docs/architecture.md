# Architecture

## Overview

```
cafe-types  (shared library)
    │
    ▼
cafe-bus  ←──────── cafe-store
  ↑  ↑  ↑
  │  │  └── cafe-llm
  │  └───── cafe-agent-runtime
  │
cafe-server ──→ cafe-web (HTTP + SSE)
  ↑
cafe-tui         (HTTP client)
cafe-telegram    (HTTP client)
```

## IPC

`cafe-bus` listens on a Unix socket (`/tmp/cafe-bus.sock` by default, configurable via
`CAFE_BUS_SOCKET`). All other services connect as clients.

## Startup order

1. `cafe-bus`
2. `cafe-store`, `cafe-llm`, `cafe-agent-runtime`  (all depend on bus)
3. `cafe-server`  (depends on bus + store)
4. `cafe-telegram` (optional, depends on server)

Managed by `process-compose` locally; systemd units for production.
