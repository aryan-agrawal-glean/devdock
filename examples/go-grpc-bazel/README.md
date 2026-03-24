# Example: Go gRPC + Bazel Monorepo

**Reference repo:** [enginoid/monorepo-base](https://github.com/enginoid/monorepo-base)
**Stack:** Go 1.21 · gRPC + Protobuf · Bazel 7 · rules_go · rules_docker

---

## What This Demonstrates

- Bazel monorepo with `//services/ping` as a gRPC service
- `bazel_target` in `devdock_config.json` — devdock runs `bazel build` automatically
- No infra dependencies — pure service-to-service gRPC
- Single-service profile (easy to extend with more services)

---

## Prerequisites

| Tool | Version | Install |
|------|---------|---------|
| Docker | 24+ | docker.com |
| Tilt | 0.33+ | tilt.dev |
| DevDock | latest | `cargo install devdock-cli` |
| Bazel | 7+ | `npm install -g @bazel/bazelisk` |

---

## Repo Layout (enginoid/monorepo-base)

```
monorepo-base/
├── WORKSPACE                  ← Bazel workspace (rules_go, rules_docker, gazelle)
├── BUILD.bazel                ← root BUILD
├── go.mod / go.sum
├── services/
│   └── ping/
│       ├── BUILD.bazel        ← go_binary + go_image + k8s_object targets
│       ├── main.go
│       └── server.go
├── cmd/
│   └── ping/                  ← CLI client to call the ping service
└── config/
```

---

## Adding DevDock to This Repo

```
monorepo-base/
├── .devdock/
│   ├── devdock.toml
│   └── profiles/
│       └── default.yaml
└── devdock/
    └── services/
        └── ping/
            └── devdock_config.json
```

---

## Running

```bash
devdock start default
open http://localhost:10350
```

---

## Expected Results

| Resource | Status | Notes |
|----------|--------|-------|
| `ping-build` | `update=ok` | Bazel build of `//services/ping` |
| `ping` | `runtime=ok` | gRPC server on port 50051 |

Verify the service is up:
```bash
# Using grpcurl (install: brew install grpcurl)
grpcurl -plaintext localhost:50051 list

# Or use the repo's built-in CLI client
bazel run //cmd/ping -- --address=localhost:50051
```

---

## Config Decisions

- **`bazel_target = "//services/ping"`** — devdock runs `bazel build //services/ping` and mounts the output binary into a container. No manual `go build` needed.
- **No infra section** — the ping service has no database or cache dependencies. Add `sql` or `redis` to the profile when you add services that need them.
- **Port 50051** — standard gRPC port. The service uses `net.Listen("tcp", ":50051")`.

---

## Extending with More Services

Add a second service to the profile:

```yaml
# .devdock/profiles/full.yaml
project: monorepo-base

services:
  ping:
    type: local
  auth:
    type: local
```

And create `devdock/services/auth/devdock_config.json`:
```json
{
  "type": "go",
  "bazel_target": "//services/auth",
  "port": "50052",
  "dependencies": ["ping"]
}
```

---

## Notes

- `rules_docker` builds images via Bazel — devdock uses the output binary directly rather than the Bazel-built image.
- The original repo deploys to GKE via `bazel run //services/ping:staging.apply`. DevDock replaces this for local development only.
- Gazelle is used for Go dependency management: `bazel run //:gazelle` to update `BUILD.bazel` files after adding imports.
