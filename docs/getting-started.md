# Getting Started with DevDock

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Tilt](https://docs.tilt.dev/install.html)
- [Bazel](https://bazel.build/) (for Bazel-managed repos)
- [Rust 1.74+](https://rustup.rs/) (to build from source)

## Installation

```bash
git clone https://github.com/askscio/devdock.git
cd devdock
cargo install --path crates/devdock-cli
```

With cloud features:
```bash
cargo install --path crates/devdock-cli --features gcp   # BigQuery, GCS, Coder
```

## Setting Up Your Repo

### 1. Initialize

```bash
cd my-monorepo
devdock init
```

This creates `.devdock/devdock.toml`. Edit it:

```toml
[project]
name = "my-project"

[cloud]
platform = "none"
```

### 2. Add Service Definitions

For each service, create `devdock/config/services/{name}/devdock_config.json`:

```json
{
  "type": "python",
  "bazel_target": "//services/api:api",
  "binary_mount": "services/api",
  "port": "8080",
  "dependencies": ["sql"],
  "env": { "PORT": "8080" },
  "healthcheck": { "test": ["CMD", "curl", "-f", "http://localhost:8080/health"] }
}
```

### 3. Create a Profile

Create `devdock/profiles/default.yaml`:

```yaml
project: my-project
infra:
  sql:
    type: local
services:
  api:
    type: local
```

### 4. Start

```bash
devdock start default
```

DevDock will resolve dependencies, build with Bazel, start Docker containers, and open the Tilt UI at http://localhost:10350.

### 5. Check and Stop

```bash
devdock status          # Check service health
devdock stop            # Stop everything
```

## Configuration Reference

### devdock.toml

| Section | Field | Default | Description |
|---------|-------|---------|-------------|
| `project` | `name` | (required) | Project name |
| `project` | `domain` | `localhost` | Local dev domain |
| `project` | `services_dir` | `devdock/config/services` | Path to service configs |
| `project` | `infra_dir` | `devdock/config/infra` | Path to infra overrides |
| `cloud` | `platform` | `none` | `none`, `google`, `aws`, `azure` |
| `cloud` | `project` | — | Cloud project/account ID |
| `telemetry` | `enabled` | `false` | Enable telemetry |
| `telemetry` | `backend` | `stdout` | `stdout`, `bigquery`, `datadog` |
| `remote` | `provider` | `ssh` | `ssh`, `coder` |
| `diagnostics` | `reporter` | `stdout` | `stdout`, `jira` |

### devdock_config.json

| Field | Required | Description |
|-------|----------|-------------|
| `type` | No (default: `custom`) | `python`, `go`, `java`, `custom` |
| `bazel_target` | No | Bazel build target |
| `binary_mount` | No | Path to built binary |
| `port` | No | Service port |
| `dependencies` | No | Other services/infra needed |
| `env` | No | Environment variables |
| `healthcheck` | No | Docker healthcheck config |
| `base_image` | No | Docker base image |
| `entrypoint` | No | Service entrypoint |

## Next Steps

- **Multiple profiles**: Create different profiles for different workflows
- **Custom infra**: Override default compose files in `devdock/config/infra/`
- **Remote dev**: Use `devdock start <profile> -m remote` for cloud workspaces
- **Plugins**: Run `devdock plugin create` to build custom backends
