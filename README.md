# DevDock

[![CI](https://github.com/askscio/devdock/actions/workflows/ci.yml/badge.svg)](https://github.com/askscio/devdock/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.74-orange.svg)](https://www.rust-lang.org)

A fast, extensible CLI for managing local and remote development environments. Works with any Bazel-managed monorepo.

## What DevDock Does

DevDock takes your service definitions (`devdock_config.json`) and profiles (`*.yaml`) and spins up a complete development environment with:

- Docker containers for your services (built via Bazel)
- Infrastructure (MySQL, Redis, OpenSearch, etc.)
- File watching and hot reload
- Remote development via SSH or Coder

**Before DevDock**: 2+ minutes of Python scripts, shell commands, and manual configuration.
**With DevDock**: Sub-60 second startup from a single command.

## Quick Start

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Tilt](https://docs.tilt.dev/install.html)
- [Bazel](https://bazel.build/) (for Bazel-managed repos)

### Install

```bash
# From source
cargo install --path crates/devdock-cli

# With GCP features (BigQuery telemetry, GCS config, Coder)
cargo install --path crates/devdock-cli --features gcp
```

### Initialize Your Repo

```bash
cd my-monorepo
devdock init
# Creates .devdock/devdock.toml
# Edit with your project settings
```

### Start

```bash
devdock start <profile>
# Generates configs, starts Tilt, services come up
```

## How It Works

### What You Commit to Your Repo

```
my-repo/
├── .devdock/
│   └── devdock.toml                    # Project config
├── devdock/
│   ├── config/
│   │   ├── services/                   # One dir per service
│   │   │   ├── api/devdock_config.json
│   │   │   └── worker/devdock_config.json
│   │   └── infra/                      # Infrastructure compose files
│   │       ├── sql/docker-compose.yaml
│   │       └── redis/docker-compose.yaml
│   └── profiles/
│       ├── default.yaml
│       └── minimal.yaml
```

### What DevDock Generates at Runtime

```
devdock start <profile>
├─ Loads devdock.toml + profile
├─ Resolves transitive dependencies (DFS)
├─ Generates: devdock.ini, temp_profile.yaml, watch files, Tiltfile
├─ Extracts shipped Starlark libs
└─ Starts Tilt with generated Tiltfile
```

Everything in `devdock/runtime_generated/` is generated fresh each start.

### Service Definition (`devdock_config.json`)

Each service needs a JSON file describing how to build and run it:

```json
{
  "type": "python",
  "bazel_target": "//services/api:api",
  "binary_mount": "services/api",
  "port": "8080",
  "dependencies": ["sql", "redis"],
  "env": {
    "PORT": "8080"
  },
  "healthcheck": {
    "test": ["CMD", "curl", "-f", "http://localhost:8080/health"]
  }
}
```

### Profile (`*.yaml`)

Profiles define which services and infrastructure to run:

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

### Project Config (`devdock.toml`)

```toml
[project]
name = "my-project"
domain = "localhost"

[cloud]
platform = "google"
project = "my-gcp-project"

[telemetry]
enabled = true
backend = "bigquery"    # or "stdout", "datadog"

[remote]
provider = "coder"      # or "ssh"

[diagnostics]
reporter = "jira"       # or "stdout", "github-issues"
```

## Configuration-Driven Backends

DevDock uses config-driven backends, not code-driven plugins. Select backends in `devdock.toml`:

| Capability | Backends | Default |
|------------|----------|---------|
| **Telemetry** | `stdout`, `bigquery`, `datadog`, `external` | `stdout` |
| **Config Storage** | `local`, `gcs`, `s3` | `local` |
| **Remote Dev** | `ssh`, `coder` | `ssh` |
| **Diagnostics** | `stdout`, `jira`, `github-issues` | `stdout` |

Cloud backends require feature flags at build time:

```bash
cargo install devdock                    # OSS: stdout, local, ssh
cargo install devdock --features gcp     # + BigQuery, GCS, Coder
cargo install devdock --features aws     # + S3, CloudWatch
```

## External Plugins

For custom backends, write a plugin in any language. DevDock communicates via JSON over stdin/stdout:

```bash
# Create a plugin from template
devdock plugin create my-prometheus --type telemetry --language python

# Configure in devdock.toml
# [telemetry]
# backend = "external"
# plugin = "~/.devdock/plugins/my-prometheus/plugin.py"
```

The generated plugin has all protocol boilerplate — just implement `handle_emit()`.

## Infrastructure

DevDock ships default compose files for common infra (MySQL, Redis, OpenSearch, Memcached, PubSub, Nginx). Override them by adding custom compose files to `devdock/config/infra/`.

Resolution order:
1. **Repo override**: `devdock/config/infra/{name}/docker-compose.yaml`
2. **Built-in default**: shipped with devdock binary

## Commands

```bash
devdock init                    # Initialize project
devdock start <profile>         # Start development environment
devdock start <profile> -s svc  # Start specific services only
devdock stop                    # Stop everything
devdock status                  # Check service health
devdock list                    # List running instances
devdock cleanup                 # Clean containers and files

# Remote development
devdock start <profile> -m remote   # Start on remote workspace
devdock sync start                  # Start file sync
devdock ports forward --all         # Forward remote ports

# Infrastructure
devdock infra sql setup             # Hydrate local SQL
devdock infra opensearch copy       # Copy search indexes

# Plugins
devdock plugin list                 # Show available backends
devdock plugin create <name>        # Create external plugin

# Diagnostics
devdock report                      # Generate bug report
devdock validate <profile>          # Validate profile
```

## Architecture

```
devdock/
├── crates/
│   ├── devdock-cli/          # CLI commands
│   ├── devdock-core/         # Core types, plugin traits, generators
│   ├── devdock-config/       # Config generation (devdock.ini, profiles)
│   ├── devdock-remote/       # Remote dev (SSH, Coder)
│   ├── devdock-sync/         # File sync daemon
│   ├── devdock-monitor/      # Startup monitoring
│   ├── devdock-state/        # SQLite state management
│   ├── devdock-telemetry/    # Telemetry collection + sync
│   └── devdock-diagnostics/  # Bug reporting
├── lib/                      # Shipped Starlark libraries
│   ├── service.Tiltfile      # Service orchestration
│   ├── docker.Tiltfile       # Dockerfile/compose generation
│   ├── bazel.Tiltfile        # Bazel build helpers
│   └── templates/            # Dockerfile templates
└── tests/fixtures/           # Test repos
```

## Development

```bash
# Build
cargo build

# Test
cargo test

# Lint
cargo clippy --workspace

# Install dev build
cargo install --path crates/devdock-cli --debug --force

# Run CI locally
cargo fmt --check && cargo clippy -- -D warnings && cargo test
```

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
