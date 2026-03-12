# DevDock

[![CI](https://github.com/askscio/devdock/actions/workflows/ci.yml/badge.svg)](https://github.com/askscio/devdock/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.74-orange.svg)](https://www.rust-lang.org)

A fast, extensible CLI for managing local and remote development environments. Works with any Bazel-managed monorepo.

## Why DevDock?

Setting up a dev environment for a monorepo with dozens of services typically involves:
- Running multiple shell scripts and Python setup tools
- Manually configuring databases, caches, and search indexes
- Waiting 2+ minutes for everything to start

DevDock replaces all of this with a single command:

```bash
devdock start <profile>
```

Sub-60 second startup. No scripts. No manual configuration.

## Architecture

```
┌──────────────────────────────────────────────────────────────────────┐
│                        devdock start <profile>                       │
│                                                                      │
│  ┌─────────────┐   ┌──────────────┐   ┌─────────────────────────┐  │
│  │ Profile YAML │──→│  Dependency  │──→│  Runtime Generation     │  │
│  │ (what to     │   │  Resolver    │   │  • devdock.ini          │  │
│  │  run)        │   │  (DFS)       │   │  • temp_profile.yaml    │  │
│  └─────────────┘   └──────────────┘   │  • Tiltfile             │  │
│                                        │  • watch_file_*.yaml    │  │
│  ┌─────────────┐   ┌──────────────┐   └────────────┬────────────┘  │
│  │ devdock.toml│──→│  Platform    │                 │               │
│  │ (project    │   │  Auth        │                 ▼               │
│  │  config)    │   │  (GCP/AWS)   │   ┌─────────────────────────┐  │
│  └─────────────┘   └──────────────┘   │  Tilt                   │  │
│                                        │  ┌───────────────────┐  │  │
│  ┌─────────────┐                      │  │ Shipped Starlark  │  │  │
│  │ devdock_    │                      │  │ Libraries         │  │  │
│  │ config.json │                      │  │ • service.Tiltfile│  │  │
│  │ (per-service│                      │  │ • docker.Tiltfile │  │  │
│  │  metadata)  │                      │  │ • bazel.Tiltfile  │  │  │
│  └─────────────┘                      │  └───────────────────┘  │  │
│                                        │           │              │  │
│                                        │           ▼              │  │
│                                        │  ┌─── Docker ────────┐  │  │
│                                        │  │ Bazel builds      │  │  │
│                                        │  │ Dockerfile gen    │  │  │
│                                        │  │ Compose gen       │  │  │
│                                        │  │ Container mgmt   │  │  │
│                                        │  └──────────────────┘  │  │
│                                        └─────────────────────────┘  │
│                                                                      │
│  ┌──────────── External Plugins (JSON stdin/stdout) ─────────────┐  │
│  │ bigquery-sync │ jira-reporter │ slack-notifier │ gcs-storage  │  │
│  └───────────────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────────────────┘
```

### How It Works

1. **You define** services (`devdock_config.json`) and profiles (`*.yaml`)
2. **DevDock generates** all runtime configs (INI, Tiltfile, watch patterns)
3. **Tilt orchestrates** Docker containers using shipped Starlark libraries
4. **Bazel builds** on host, binaries volume-mounted into containers
5. **External plugins** handle telemetry sync, bug reporting, notifications

### What's in Your Repo vs What DevDock Ships

```
YOUR REPO (committed):                  DEVDOCK BINARY (shipped):
├── .devdock/devdock.toml               ├── Starlark libraries
├── devdock/config/services/            │   ├── service.Tiltfile
│   ├── api/devdock_config.json         │   ├── docker.Tiltfile
│   └── worker/devdock_config.json      │   ├── bazel.Tiltfile
├── devdock/config/infra/               │   └── templates/*.tpl
│   ├── sql/docker-compose.yaml         ├── Default infra compose
│   └── redis/docker-compose.yaml       │   ├── sql, redis, opensearch
├── devdock/profiles/                   │   ├── memcached, pubsub
│   ├── default.yaml                    │   └── nginx
│   └── minimal.yaml                    └── Plugin SDK
└── source code (anywhere)
```

## Quick Start

### Prerequisites

- [Docker](https://docs.docker.com/get-docker/)
- [Tilt](https://docs.tilt.dev/install.html)
- [Bazel](https://bazel.build/) (for Bazel-managed repos)

### Install

```bash
# From source
git clone https://github.com/askscio/devdock.git
cd devdock
cargo install --path crates/devdock-cli
```

### Initialize Your Repo

```bash
cd my-monorepo
devdock init
# Edit .devdock/devdock.toml with your project settings
```

### Create a Service Definition

```bash
mkdir -p devdock/config/services/api
```

`devdock/config/services/api/devdock_config.json`:
```json
{
  "type": "python",
  "bazel_target": "//services/api:api",
  "binary_mount": "services/api",
  "port": "8080",
  "dependencies": ["sql", "redis"],
  "env": { "PORT": "8080" },
  "healthcheck": {
    "test": ["CMD", "curl", "-f", "http://localhost:8080/health"]
  }
}
```

### Create a Profile

`devdock/profiles/default.yaml`:
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
```

### Start

```bash
devdock start default
# → Generates configs
# → Builds with Bazel
# → Starts Docker containers
# → Tilt UI at http://localhost:10350
```

## Configuration

### Project Config (`devdock.toml`)

```toml
[project]
name = "my-project"
domain = "localhost"
services_dir = "devdock/config/services"    # where service configs live
infra_dir = "devdock/config/infra"          # where infra compose files live

[cloud]
platform = "google"           # google, aws, azure, none
project = "my-gcp-project"
region = "us-central1"

[telemetry]
enabled = true
backend = "stdout"            # stdout, bigquery, datadog, external

[remote]
provider = "ssh"              # ssh, coder

[diagnostics]
reporter = "stdout"           # stdout, jira
jira_url = "https://your-org.atlassian.net"
jira_email = "bot@your-org.com"
jira_project = "ENG"
secret_project = "your-gcp-project"
```

### Service Config (`devdock_config.json`)

| Field | Required | Description |
|-------|----------|-------------|
| `type` | No | `python`, `go`, `java`, `custom` (default: `custom`) |
| `bazel_target` | No | Bazel build target (e.g., `//services/api:api`) |
| `binary_mount` | No | Path to built binary relative to repo root |
| `port` | No | Service port |
| `dependencies` | No | Other services/infra this service needs |
| `env` | No | Environment variables |
| `healthcheck` | No | Docker healthcheck config |
| `requirements` | No | Python requirements paths (linux, darwin, wheels) |

### Profile (`*.yaml`)

```yaml
project: my-project

infra:
  sql:
    type: local              # local: Docker container, remote: kubectl port-forward
  redis:
    type: local

services:
  api:
    type: local              # local: Bazel build + Docker, remote: port-forward
    env:
      DEBUG_MODE: "true"
    hot_reload: enable
  worker:
    type: local

# Optional: multi-project support
multi_project:
  project: secondary-project
  services:
    internal_admin:
      type: remote
```

## External Plugins

DevDock uses external plugins for cloud integrations. Plugins are standalone executables that communicate via JSON over stdin/stdout.

### Available Plugins

| Plugin | Location | What it does |
|--------|----------|-------------|
| `bigquery-sync` | `plugins/common/` | Sync telemetry to BigQuery |
| `jira-reporter` | `plugins/common/` | Create Jira tickets for bug reports |
| `slack-notifier` | `plugins/common/` | Send Slack notifications |
| `gcs-storage` | `plugins/glean/` | Read config from GCS buckets |
| `s3-storage` | `plugins/glean/` | Read config from S3 buckets |

### Install a Plugin

```bash
cargo install --path plugins/common/bigquery-sync
```

### Create Your Own Plugin

```bash
devdock plugin create my-metrics --type telemetry --language python
# Generates ~/.devdock/plugins/my-metrics/ with boilerplate
```

Plugin templates available for **Rust**, **Python**, **Go**, and **Bash**. Rust plugins use the `devdock-plugin-sdk` crate for type-safe, native-performance plugins.

### Plugin Protocol

```
devdock binary                    plugin executable
    │                                  │
    ├─ {"method":"initialize",         │
    │   "config":{...}} ──────────────→│
    │   ←────────────────────────────── {"status":"ok"}
    │                                  │
    ├─ {"method":"emit",               │
    │   "event":{...}} ───────────────→│
    │   ←────────────────────────────── {"status":"ok"}
    │                                  │
    ├─ {"method":"shutdown"} ─────────→│
    │   ←────────────────────────────── {"status":"ok"}
```

## Infrastructure

DevDock ships default compose files for common infrastructure:

| Component | Image | Port |
|-----------|-------|------|
| MySQL | `mysql:8.0` | 3306 |
| Redis | `redis:7-alpine` | 6379 |
| OpenSearch | `opensearch:2.11` | 9200 |
| Memcached | `memcached:1.6` | 11211 |
| PubSub | GCP emulator | 8085 |
| Nginx | `nginx:alpine` | 8321 |

Override any component by adding a custom `docker-compose.yaml` to `devdock/config/infra/{name}/`.

## Commands

```bash
# Core
devdock init                        # Initialize project
devdock start <profile>             # Start development environment
devdock start <profile> -s svc      # Start specific services only
devdock stop                        # Stop everything
devdock status                      # Check service health
devdock list                        # List all instances
devdock cleanup                     # Clean containers and files
devdock validate <profile>          # Validate profile config

# Remote development
devdock start <profile> -m remote   # Start on remote workspace
devdock sync start                  # Start file sync
devdock ports forward --all         # Forward remote ports

# Infrastructure
devdock infra sql setup             # Hydrate local SQL
devdock infra opensearch copy       # Copy search indexes

# Plugins
devdock plugin list                 # Show backends + installed plugins
devdock plugin create <name>        # Create external plugin

# Diagnostics
devdock report                      # Generate and submit bug report
devdock docker check                # Check Docker status
devdock profile list                # List available profiles

# Logging
devdock logs enable                 # Set up logging (via plugin)
devdock logs status                 # Check logging status
```

## Repo Structure

```
devdock/
├── crates/                     # Rust source (zero cloud-specific code)
│   ├── devdock-cli/            #   CLI commands, entry point
│   ├── devdock-core/           #   Core types, Tiltfile generator, plugin traits
│   ├── devdock-config/         #   Config generation (devdock.ini, profiles)
│   ├── devdock-remote/         #   Remote dev (SSH, Coder)
│   ├── devdock-sync/           #   File sync daemon
│   ├── devdock-monitor/        #   Startup health monitoring
│   ├── devdock-state/          #   SQLite state management
│   ├── devdock-telemetry/      #   Telemetry collection (sync via plugins)
│   ├── devdock-diagnostics/    #   Bug reporting
│   └── devdock-codegen/        #   Config codegen
├── lib/                        # Shipped Starlark libraries
│   ├── service.Tiltfile        #   Service orchestration
│   ├── docker.Tiltfile         #   Dockerfile/compose generation
│   ├── bazel.Tiltfile          #   Bazel build helpers
│   └── templates/              #   Dockerfile templates (Python, Go, Java)
├── infra/                      # Default infra compose files
├── plugins/
│   ├── sdk/                    #   Plugin SDK (Rust crate)
│   ├── common/                 #   Community plugins (BigQuery, Jira, Slack)
│   ├── glean/                  #   Glean-specific plugins (GCS, S3)
│   ├── templates/              #   Plugin scaffolding (Rust, Python, Go, Bash)
│   └── examples/               #   Example plugins (Prometheus, Slack)
├── tests/fixtures/             # Test repos
├── docs/                       # Documentation
├── CLAUDE.md                   # Claude Code development guide
├── CONTRIBUTING.md
├── LICENSE-MIT
└── LICENSE-APACHE
```

## Development

### With Claude Code

DevDock is a Claude-friendly repo with built-in skills for agentic development:

```bash
/build-and-test              # Build, clippy, test, install
/test-profile qe             # E2E test a profile against scio
/check-tilt-status           # Check running Tilt resources
/setup-scio-test             # Set up scio repo for testing
```

See `CLAUDE.md` for the full development guide.

### Manual

```bash
cargo build                  # Build all crates
cargo test                   # Run tests (141 tests)
cargo clippy --workspace     # Lint (0 warnings required)
cargo install --path crates/devdock-cli --debug --force  # Install dev build
```

### Building Plugins

```bash
cd plugins/common/bigquery-sync
cargo build
cargo install --path .
```

## Deployment: Binary Releases

### OSS Binary (public)

The devdock binary has **zero cloud-specific code**. It contains:
- Core CLI commands
- Telemetry collection (sync via external plugins)
- Shipped Starlark libraries
- Default infra compose files
- Plugin SDK protocol handler

```bash
# Install from source (OSS)
cargo install --path crates/devdock-cli

# Or download pre-built binary from GitHub Releases
curl -L https://github.com/askscio/devdock/releases/latest/download/devdock-$(uname -s)-$(uname -m) -o devdock
chmod +x devdock
```

### Organization-Specific Plugins (separate repo)

Cloud integrations ship as separate plugin binaries. For example, Glean's plugins
live in a private repo (`devdock-glean-plugins`) and are installed separately:

```bash
# Install devdock (same binary for everyone)
cargo install devdock

# Install organization plugins (from private repo)
cargo install --git https://github.com/your-org/devdock-plugins devdock-plugin-bigquery-sync
cargo install --git https://github.com/your-org/devdock-plugins devdock-plugin-jira-reporter
cargo install --git https://github.com/your-org/devdock-plugins devdock-plugin-gcs-storage
```

The plugin binaries go to `~/.cargo/bin/` (in PATH). DevDock finds them by name.

### Plugin Repo Structure

```
your-org/devdock-plugins/           # Private repo
├── Cargo.toml                      # Workspace
├── bigquery-sync/
│   ├── Cargo.toml                  # depends on devdock-plugin-sdk
│   └── src/main.rs
├── jira-reporter/
│   ├── Cargo.toml
│   └── src/main.rs
├── gcs-storage/
│   ├── Cargo.toml
│   └── src/main.rs
└── s3-storage/
    ├── Cargo.toml
    └── src/main.rs
```

Each plugin depends only on `devdock-plugin-sdk` (published to crates.io), NOT on devdock internals.

### How It All Connects

```
User's Machine
├── devdock (binary)                     ← from github.com/askscio/devdock
├── devdock-plugin-bigquery-sync         ← from your-org/devdock-plugins
├── devdock-plugin-jira-reporter         ← from your-org/devdock-plugins
└── devdock-plugin-gcs-storage           ← from your-org/devdock-plugins

devdock.toml
├── [telemetry]
│   backend = "external"
│   plugin = "devdock-plugin-bigquery-sync"   ← found in $PATH
├── [diagnostics]
│   reporter = "jira"                          ← built-in config-driven
└── [cloud]
    storage = "external"
    plugin = "devdock-plugin-gcs-storage"      ← found in $PATH
```

When `devdock start` runs, it spawns plugin binaries as subprocesses and communicates
via JSON stdin/stdout. The main binary never imports cloud SDK code.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and how to add new backends.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
