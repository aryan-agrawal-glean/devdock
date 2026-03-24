# Example: Data Pipeline + Custom Plugin

**Pattern:** Python ingestion + processing pipeline with streaming infra + lifecycle telemetry plugin
**Stack:** Python 3.11 · OpenSearch · Pub/Sub (local emulator) · PostgreSQL · Custom DevDock plugin

---

## What This Demonstrates

- Multi-infra setup: PostgreSQL + OpenSearch + Pub/Sub emulator
- Service dependency chain: `processor` waits for `ingester` which waits for infra
- **Custom lifecycle plugin** that logs session telemetry (`after_tilt`, `before_stop`, `on_error`)
- How to wire a plugin into `[plugins.hooks]` in `devdock.toml`
- Profile with all infra vs lightweight profile for just the core pipeline

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Docker | 24+ | docker.com |
| Tilt | 0.33+ | tilt.dev |
| DevDock | latest | `cargo install devdock-cli` |
| Python | 3.11+ | python.org (to build the plugin) |

---

## Typical Repo Layout

```
data-platform/
├── services/
│   ├── ingester/          ← HTTP API that accepts data and publishes to Pub/Sub
│   │   ├── main.py
│   │   └── requirements.txt
│   └── processor/         ← Subscribes from Pub/Sub, enriches with OpenSearch, writes to Postgres
│       ├── main.py
│       └── requirements.txt
└── Dockerfile             ← shared base image
```

---

## Adding DevDock to This Repo

```
data-platform/
├── .devdock/
│   ├── devdock.toml            ← includes [plugins.hooks] for session-logger
│   └── profiles/
│       ├── default.yaml        ← all infra + both services
│       └── ingester-only.yaml  ← just ingester + pubsub (for API testing)
├── devdock/
│   └── services/
│       ├── ingester/
│       │   └── devdock_config.json
│       └── processor/
│           └── devdock_config.json
└── plugins/
    └── session-logger/         ← custom telemetry plugin
        ├── main.py
        └── README.md
```

---

## Running

```bash
# Build the plugin first
cd plugins/session-logger
pip install -r requirements.txt
chmod +x main.py

# Start the full pipeline
devdock start default

# Start just the ingester for API development
devdock start ingester-only

open http://localhost:10350
```

---

## Expected Results

### `devdock start default`

| Resource | Status | URL |
|----------|--------|-----|
| `sql` (postgres) | `runtime=ok` | localhost:5432 |
| `opensearch` | `runtime=ok` | http://localhost:9200 |
| `pubsub` (emulator) | `runtime=ok` | localhost:8085 |
| `ingester` | `runtime=ok` | http://localhost:8000 |
| `processor` | `runtime=ok` | (no HTTP — subscriber process) |

The `session-logger` plugin fires automatically:
- **`after_tilt`**: logs session start to stdout (or your telemetry backend)
- **`before_stop`**: logs session duration and service health summary
- **`on_error`**: logs which service errored and the error context

Verify:
```bash
# Ingest a data event
curl -X POST http://localhost:8000/ingest \
  -H "Content-Type: application/json" \
  -d '{"id": "123", "payload": {"value": 42}}'

# Check processor picked it up (watch Tilt logs for the processor service)

# Query OpenSearch for enriched data
curl http://localhost:9200/events/_search?q=id:123
```

---

## Plugin: session-logger

The `session-logger` plugin demonstrates the DevDock plugin protocol.
It receives lifecycle events and logs them.

See [`plugins/session-logger/README.md`](./plugins/session-logger/README.md) for details.

**Wire-up in `devdock.toml`:**
```toml
[plugins.hooks]
after_tilt   = ["./plugins/session-logger/main.py"]
before_stop  = ["./plugins/session-logger/main.py"]
on_error     = ["./plugins/session-logger/main.py"]
```

---

## Config Decisions

- **`processor` has no `port`** — it's a subscriber process, not an HTTP server. DevDock manages its container lifecycle anyway.
- **`processor` depends on `ingester`** — ensures the ingester's topics and schemas are created before the processor subscribes.
- **OpenSearch on port 9200** — devdock ships infra compose for `opensearch` (single-node, no auth for local dev).
- **Pub/Sub emulator on port 8085** — devdock ships infra compose for `pubsub` using the Google Cloud Pub/Sub emulator image.
- **Plugin uses `./` relative path** — plugins are run from the repo root; relative paths work for plugins checked into the repo.

---

## Notes

- For production telemetry, replace `session-logger` with a real telemetry plugin (e.g., BigQuery sync from `plugins/telemetry/bigquery-sync/`).
- OpenSearch by default has security disabled in the shipped devdock compose (`DISABLE_SECURITY_PLUGIN=true`). Enable it for staging environments.
- The Pub/Sub emulator requires creating topics/subscriptions before use. Add a `before_tilt` hook or startup script to initialize them.
