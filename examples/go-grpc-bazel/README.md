# Example: Go gRPC + Bazel Monorepo

**Stack:** Go · HTTP/gRPC service · Bazel (Bzlmod) · rules_go 0.60+

> **Verified:** `ping` service reaches `runtime=ok`. `/health` returns `{"status":"ok"}`, `/ping` returns `pong`. Binary cross-compiled Linux ARM64 ELF and mounted correctly.

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

## Repo Layout

```
my-go-monorepo/
├── MODULE.bazel               ← Bzlmod deps (rules_go with repo_name = "io_bazel_rules_go")
├── services/
│   └── ping/
│       ├── BUILD              ← go_binary(name = "ping", ...)
│       └── main.go            ← HTTP/gRPC server
└── devdock/
    └── services/
        └── ping/
            ├── devdock_config.json
            └── entrypoint.sh
```

### MODULE.bazel requirements for Bzlmod

```starlark
# repo_name alias required — devdock's lib/bazel.Tiltfile references
# @io_bazel_rules_go//go/toolchain:linux_arm64 for cross-compilation
bazel_dep(name = "rules_go", version = "0.60.0", repo_name = "io_bazel_rules_go")
bazel_dep(name = "rules_cc", version = "0.1.1")  # required by rules_go on Bazel 9+
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
| `ping-bazel-build` | `update=ok` | Bazel cross-compiles `//services/ping` → Linux ARM64 ELF |
| `ping` | `runtime=ok` | HTTP service on port 50051 |

Verify the service is up:
```bash
curl http://localhost:50051/health
# → {"status":"ok","time":"..."}

curl http://localhost:50051/ping
# → pong
```

---

## Config Decisions

- **`bazel_target = "//services/ping"`** — devdock runs `bazel build //services/ping` (with cross-compilation flags on macOS) and mounts the Linux ARM64 binary into the container.
- **`target_is_directory = "True"`** — tells devdock the bazel output is a directory (rules_go uses `{name}_/` convention). devdock rsyncs the whole directory to `~/.devdock/ping/`.
- **Entrypoint at `/app/ping/ping`** — devdock mounts `~/.devdock/{service_name}` to `/app/{service_name}`. The binary inside is named after the Bazel target. So for service `ping` with `go_binary(name = "ping")`, the binary is at `/app/ping/ping` inside the container.
- **No infra section** — pure service, no database or cache dependencies.
- **Port 50051** — standard gRPC/HTTP port.

---

## Extending with More Services

Add a second service to the profile:

```yaml
# .devdock/profiles/full.yaml
project: my-go-monorepo

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
  "target_is_directory": "True",
  "port": "50052",
  "dependencies": ["ping"],
  "entrypoint": {"script": "entrypoint.sh"},
  "healthcheck": {"test": ["CMD-SHELL", "wget -qO- http://localhost:50052/health || exit 1"]}
}
```

With `devdock/services/auth/entrypoint.sh`:
```sh
#!/bin/sh
set -e
exec /app/auth/auth
```

---

## Notes

- On macOS, devdock automatically cross-compiles to Linux ARM64 via `--platforms=@io_bazel_rules_go//go/toolchain:linux_arm64`. No manual configuration needed.
- The `repo_name = "io_bazel_rules_go"` alias in `MODULE.bazel` is required for the cross-compilation platform flag to resolve in Bzlmod.
- The `bazel-bin` workspace symlink is used (not `bazel info bazel-bin`) to correctly locate cross-compiled outputs.
- Gazelle can manage BUILD files: `bazel run //:gazelle` to update after adding Go imports.
