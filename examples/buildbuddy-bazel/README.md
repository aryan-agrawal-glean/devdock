# Example: BuildBuddy — Go + React + Bazel

**Reference repo:** [buildbuddy-io/buildbuddy](https://github.com/buildbuddy-io/buildbuddy)
**Stack:** Go 1.21 · React (bundled into Go binary) · Bazel 7 (Bzlmod) · Optional MySQL/PostgreSQL

---

## What This Demonstrates

- Large Bazel monorepo with a single Go binary (`//server/cmd/buildbuddy`)
- Multi-port service: HTTP (8080) + gRPC Build Event Stream (1985)
- Config file passed via environment variable (not inline env vars)
- Optional SQL infra (Postgres) for persistent storage
- Profile variants: `standalone` (disk storage, no DB) vs `full` (with Postgres)

---

## About BuildBuddy

BuildBuddy is an open source Bazel build event viewer, result store, and remote cache. Its core is a single Go binary that:
- Serves a React web UI on port 8080
- Accepts Bazel Build Event Protocol (BES) events on gRPC port 1985
- Stores build results on disk (default) or in MySQL/PostgreSQL

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Docker | 24+ | docker.com |
| Tilt | 0.33+ | tilt.dev |
| DevDock | latest | `cargo install devdock-cli` |
| Bazel | 7+ | `npm install -g @bazel/bazelisk` |

---

## Repo Layout (buildbuddy-io/buildbuddy)

```
buildbuddy/
├── WORKSPACE / MODULE.bazel     ← Bazel workspace (Bzlmod)
├── server/
│   ├── cmd/
│   │   └── buildbuddy/          ← Main Go binary (HTTP + gRPC server)
│   │       ├── main.go
│   │       └── BUILD
│   ├── buildbuddy_server/       ← Core server logic
│   └── ...
├── app/                         ← React frontend (bundled into Go binary via Bazel)
├── config/
│   └── buildbuddy.local.yaml    ← Local development config
└── go.mod / go.sum
```

---

## Adding DevDock to This Repo

```
buildbuddy/
├── .devdock/
│   ├── devdock.toml
│   └── profiles/
│       ├── standalone.yaml      ← app only (disk storage)
│       └── full.yaml            ← app + postgres
└── devdock/
    └── services/
        └── app/
            └── devdock_config.json
```

---

## Running

```bash
# Standalone mode (no database, disk storage only)
devdock start standalone

# Full mode with Postgres for persistent build history
devdock start full

# Open Tilt dashboard
open http://localhost:10350

# Once running, point Bazel at your local BuildBuddy:
# In your project's .bazelrc:
# build --bes_results_url=http://localhost:8080/invocation/
# build --bes_backend=grpc://localhost:1985
```

---

## Expected Results

### `devdock start standalone`

| Resource | Status | URL |
|----------|--------|-----|
| `app-build` | `update=ok` | (Bazel build of `//server/cmd/buildbuddy`) |
| `app` | `runtime=ok` | http://localhost:8080 |

### `devdock start full`

| Resource | Status | URL |
|----------|--------|-----|
| `sql` (postgres) | `runtime=ok` | localhost:5432 |
| `app-build` | `update=ok` | |
| `app` | `runtime=ok` | http://localhost:8080 |

Verify:
```bash
# Web UI
open http://localhost:8080

# gRPC health check
grpcurl -plaintext localhost:1985 grpc.health.v1.Health/Check

# Point a Bazel build at it
cd /your/other/project
bazel build //... --bes_backend=grpc://localhost:1985 \
                  --bes_results_url=http://localhost:8080/invocation/
# Then open the printed URL to see your build results
```

---

## Config Decisions

- **`bazel_target = "//server/cmd/buildbuddy"`** — the single Go binary that serves everything (HTTP + gRPC + React UI).
- **Two ports** — 8080 for HTTP/React UI, 1985 for the gRPC Build Event Stream (BES). Both are served by the same binary. The `port` field in devdock_config.json is the primary HTTP port; the gRPC port is also exposed via Docker.
- **`BUILDBUDDY_LOCAL_CONFIG`** — BuildBuddy reads config from a YAML file. The path is passed via this env var pointing to `config/buildbuddy.local.yaml` in the repo.
- **Disk storage in standalone mode** — no database needed; build results stored in `/tmp`. Good for quick demos.
- **Postgres in full mode** — persistent build history survives restarts. The connection string uses the `sql.devdock.local` DNS alias.

---

## Notes

- The React frontend is embedded into the Go binary at build time via Bazel `go_embed_data` rules — no separate frontend service needed.
- BuildBuddy uses Bzlmod (`MODULE.bazel`) as well as a legacy `WORKSPACE` file. Both are supported by Bazel 7+.
- For remote caching and remote execution, BuildBuddy requires an enterprise license. The open source version supports BES + result store only.
- The `config/buildbuddy.local.yaml` in the repo contains sensible defaults for local development (1GB disk cache, disk storage, etc.).
