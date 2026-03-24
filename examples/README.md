# DevDock Examples

Real-world configurations showing how to add DevDock to different project types.
Each example is a drop-in config set — copy the files into your repo and adjust names/ports.

## Examples

| # | Folder | Reference Repo | Stack | Bazel? |
|---|--------|----------------|-------|--------|
| 1 | [fastapi-react-postgres](./fastapi-react-postgres/) | [tiangolo/full-stack-fastapi-template](https://github.com/tiangolo/full-stack-fastapi-template) | FastAPI + React + PostgreSQL | No |
| 2 | [go-grpc-bazel](./go-grpc-bazel/) | [enginoid/monorepo-base](https://github.com/enginoid/monorepo-base) | Go gRPC + Bazel | Yes |
| 3 | [buildbuddy-bazel](./buildbuddy-bazel/) | [buildbuddy-io/buildbuddy](https://github.com/buildbuddy-io/buildbuddy) | Go + React (embedded) + Bazel | Yes |
| 4 | [django-celery-worker](./django-celery-worker/) | Django + Celery pattern | Python worker + Redis + Postgres | No |
| 5 | [data-pipeline-plugin](./data-pipeline-plugin/) | Custom data pipeline | Python + OpenSearch + Pub/Sub + plugin | No |

## How to Use an Example

1. **Copy** `.devdock/` and `devdock/` into your repo root
2. **Edit** `.devdock/devdock.toml` — set `[project] name`
3. **Edit** `.devdock/profiles/default.yaml` — add/remove services to match your repo
4. **Edit** each `devdock/services/<name>/devdock_config.json` — set ports, env vars, deps
5. **Run** `devdock start default`

## File Layout Reference

```
your-repo/
├── .devdock/
│   ├── devdock.toml            ← project name, cloud platform, plugins
│   └── profiles/
│       └── default.yaml        ← which services/infra to start
└── devdock/
    └── services/
        └── <service-name>/
            └── devdock_config.json   ← port, type, dependencies, build command
```

## Key Config Fields

**`.devdock/devdock.toml`**
```toml
[project]
name = "my-project"         # used in Tilt resource labels

[cloud]
platform = "none"           # "none" | "google" | "aws"

[plugins.hooks]             # optional: wire in lifecycle plugins
after_tilt = ["./plugins/my-plugin"]
```

**`devdock/services/<name>/devdock_config.json`**
```json
{
  "type": "python",                         // python | node | go | java | custom
  "port": "8000",                           // container port
  "bazel_target": "//services/api:deploy",  // for Bazel repos
  "build_command": ["npm", "run", "build"], // for non-Bazel repos
  "build_output": "dist/",                  // artifact path to mount
  "binary_mount": "/app/dist",              // mount path inside container
  "dependencies": ["sql", "redis"],         // other services that must start first
  "env": { "DATABASE_URL": "..." }          // injected into the container
}
```

**`.devdock/profiles/default.yaml`**
```yaml
project: my-project

infra:
  sql:
    type: local
  redis:
    type: local

services:
  api:
    type: local
  worker:
    type: local
```
