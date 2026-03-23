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

DevDock uses an explicit **lifecycle state machine** to drive startup. Each state transition fires plugin hooks, making the pipeline fully extensible:

```
devdock start <profile>
│
├── Initializing ─── Parse CLI args, find workspace root
│
├── ProfileLoaded ── Load profile YAML, validate, resolve dependencies
│   └── hook: after_profile_loaded
│
├── PluginsReady ─── Load devdock.toml, spawn plugin processes
│   └── hook: before_infra_ready (auth, cloud credentials)
│
├── InfraReady ───── Docker check, networks, port allocation
│   └── hook: before_config_generated (fetch remote configs)
│
├── ConfigReady ──── Generate temp_profile.yaml, devdock.ini
│   └── hook: after_config_generated (enrich configs)
│
├── TiltfileReady ── Discover services, resolve deps, generate Tiltfile
│   └── hook: before_tilt (last-minute config)
│
├── TiltRunning ──── Spawn `tilt up`, extract Starlark libs
│   └── hook: after_tilt (telemetry: session started)
│
├── ResourcesStarting ── Per-resource startup with hooks:
│   │   hook: before_resource_start(name, kind)
│   │   hook: after_resource_start(name, kind)
│   └── e.g., sql-hydrator runs after SQL becomes healthy
│
├── Running ──────── All resources healthy, steady state
│   └── hook: after_ready (notifications: "DevDock ready in 3m")
│
├── Stopping ─────── Ctrl+C or Tilt exits
│   └── hook: before_stop (final telemetry flush)
│
└── Stopped ──────── Cleanup complete
    └── hook: after_stop

Any state → Failed → hook: on_error → Stopping
```

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
# → Lifecycle machine runs through all states
# → Generates configs, builds with Bazel, starts Docker containers
# → Tilt UI at http://localhost:10350
```

## Configuration

### Project Config (`devdock.toml`)

```toml
[project]
name = "my-project"
domain = "localhost"
services_dir = "devdock/config/services"
infra_dir = "devdock/config/infra"

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

[plugins.hooks]
before_infra_ready = ["my-auth-plugin"]
after_config_generated = ["my-config-plugin"]
after_tilt = ["devdock-plugin-bigquery-sync"]
after_ready = ["devdock-plugin-slack-notifier"]
before_stop = ["devdock-plugin-bigquery-sync"]
on_error = ["devdock-plugin-jira-reporter"]
on_report = ["devdock-plugin-jira-reporter"]

[plugins.resource_hooks.sql]
after_start = ["devdock-plugin-sql-hydrator"]

[plugins.resource_hooks.opensearch]
after_start = ["devdock-plugin-opensearch-hydrator"]
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
```

## Lifecycle Hooks

DevDock fires **14 plugin hooks** at state transitions. Plugins only need to handle hooks they care about.

| Hook | When | Example Use |
|------|------|-------------|
| `after_profile_loaded` | Profile YAML loaded | Validate custom fields |
| `before_infra_ready` | Before Docker/ports | Cloud auth, credentials |
| `after_infra_ready` | After Docker/ports | Custom network setup |
| `before_config_generated` | Before config files | Fetch remote configs (GCS/S3) |
| `after_config_generated` | After config files | Write devdock.ini, enrich configs |
| `before_tilt` | Before Tilt spawns | Last-minute config |
| `after_tilt` | After Tilt is up | Telemetry: session started |
| `before_resource_start` | Before resource starts | Per-resource setup |
| `after_resource_start` | After resource healthy | SQL hydration, search reindex |
| `after_ready` | All resources healthy | Slack: "DevDock ready in 3m" |
| `before_stop` | Ctrl+C / exit signal | Final telemetry flush |
| `after_stop` | After cleanup | Post-cleanup actions |
| `on_error` | Unrecoverable error | Create Jira ticket |
| `on_report` | `devdock report` | Create bug report |

## MCP Server

DevDock includes an MCP (Model Context Protocol) server for AI agent integration. This lets Claude Code, Cursor, Codex, and other AI tools control DevDock directly.

### Setup

Add to your project's `.mcp.json`:

```json
{
  "mcpServers": {
    "devdock": {
      "command": "devdock",
      "args": ["mcp", "serve"]
    }
  }
}
```

### Available Tools

| Tool | Description |
|------|-------------|
| `devdock_start` | Start a profile (dry-run or real) |
| `devdock_stop` | Stop current instance |
| `devdock_status` | Get all resource health from Tilt API |
| `devdock_resource_status` | Get health of a specific resource |
| `devdock_logs` | Get recent logs for a service |
| `devdock_profile_list` | List available profiles |
| `devdock_profile_show` | Show profile details |
| `devdock_validate` | Validate a profile |
| `devdock_plugin_create` | Scaffold a new plugin |
| `devdock_plugin_list` | List installed plugins |
| `devdock_lifecycle_state` | Get current lifecycle state |
| `devdock_config_show` | Show devdock.toml contents |

## Plugins

DevDock uses external plugins for cloud integrations. Plugins are standalone executables that communicate via JSON over stdin/stdout.

### Available Plugins

| Plugin | Location | Hooks |
|--------|----------|-------|
| `bigquery-sync` | `plugins/telemetry/` | `after_tilt`, `after_ready`, `before_stop` |
| `slack-notifier` | `plugins/notifications/` | `after_ready`, `on_error` |
| `jira-reporter` | `plugins/diagnostics/` | `on_error`, `on_report` |

Organization-specific plugins live in separate repos. For example, your org's plugins can live in a private `devdock-plugins` repo and be installed via `cargo install`.

### Install a Plugin

```bash
cargo install --path plugins/telemetry/bigquery-sync
```

### Create Your Own Plugin

```bash
devdock plugin create my-metrics --type telemetry --language python
```

Plugin templates available for **Rust**, **Python**, **Go**, and **Bash**.

### Plugin Protocol

```
devdock binary                    plugin executable
    │                                  │
    ├─ {"method":"initialize",         │
    │   "config":{...}} ──────────────→│
    │   ←────────────────────────────── {"status":"ok"}
    │                                  │
    ├─ {"method":"after_tilt",         │
    │   "params":{...}} ──────────────→│
    │   ←────────────────────────────── {"status":"ok"}
    │                                  │
    ├─ {"method":"shutdown"} ─────────→│
    │   ←────────────────────────────── {"status":"ok"}
```

## Infrastructure

DevDock is bring-your-own-infra. You define infrastructure components in your profile YAML and provide either a `devdock_config.json` or a `docker-compose.yaml` for each one.

### How infra is resolved

For each infra component in your profile, DevDock checks (in order):

1. **Custom Tiltfile** — `devdock/infra/{name}/Tiltfile` (full control)
2. **Custom compose** — `devdock/infra/{name}/docker-compose.yaml` (your own image/config)
3. **Config-generated** — auto-generated from `devdock_config.json` with `"kind": "infra"`
4. **Built-in default** — shipped with the devdock binary (extracted at runtime)

### Built-in defaults

DevDock ships default compose files for common infrastructure. These are compiled into the binary and used as a fallback when you don't provide your own:

| Component | Image | Port |
|-----------|-------|------|
| MySQL | `mysql:8.0` | 3306 |
| Redis | `redis:7-alpine` | 6379 |
| OpenSearch | `opensearch:2.11` | 9200 |
| Memcached | `memcached:1.6` | 11211 |
| PubSub | GCP emulator | 8085 |
| Nginx | `nginx:alpine` | 8321 |

To override a default, add your own `docker-compose.yaml` to `devdock/infra/{name}/` in your repo. The infra directory location is configurable via `infra_dir` in `devdock.toml`.

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

# Plugins
devdock plugin list                 # Show installed plugins
devdock plugin create <name>        # Create external plugin

# MCP server
devdock mcp serve                   # Start MCP server (stdin/stdout)

# Diagnostics
devdock report                      # Generate and submit bug report
devdock docker check                # Check Docker status
devdock profile list                # List available profiles
devdock monitor                     # Monitor startup health
```

## Repo Structure

```
devdock/
├── crates/                         # Rust source (12 crates, zero cloud-specific code)
│   ├── devdock-cli/                #   CLI entry point, commands
│   ├── devdock-lifecycle/          #   Lifecycle state machine, hooks, contexts
│   ├── devdock-mcp/                #   MCP server for AI agent integration
│   ├── devdock-core/               #   Core types, Tiltfile generator, plugin manager
│   ├── devdock-config/             #   Profile loading, validation, watch patterns
│   ├── devdock-state/              #   SQLite instance tracking
│   ├── devdock-codegen/            #   Config code generation
│   ├── devdock-remote/             #   Remote dev (SSH, Coder)
│   ├── devdock-sync/               #   File sync daemon
│   ├── devdock-monitor/            #   Startup health monitoring
│   ├── devdock-telemetry/          #   Telemetry collection + file emitter
│   └── devdock-diagnostics/        #   Bug reporting
├── plugins/
│   ├── sdk/                        #   Plugin SDK (Rust crate, JSON protocol)
│   ├── telemetry/
│   │   └── bigquery-sync/          #   Telemetry → BigQuery
│   ├── notifications/
│   │   └── slack-notifier/         #   Slack notifications
│   ├── diagnostics/
│   │   └── jira-reporter/          #   Jira bug reports
│   ├── templates/                  #   Plugin scaffolding (Rust, Python, Go, Bash)
│   └── examples/                   #   Example plugins (Prometheus, Slack)
├── skills/                         #   Claude Code skills
│   ├── devdock-start.md
│   ├── devdock-debug.md
│   ├── devdock-plugin-create.md
│   └── devdock-profile-create.md
├── lib/                            #   Shipped Starlark libraries
│   ├── service.Tiltfile
│   ├── docker.Tiltfile
│   ├── bazel.Tiltfile
│   └── templates/                  #   Dockerfile templates
├── infra/                          #   Default infra compose files
├── tests/fixtures/                 #   Test repos
├── docs/                           #   Documentation
├── .mcp.json                       #   MCP server config for AI agents
├── CLAUDE.md                       #   Claude Code development guide
└── CONTRIBUTING.md
```

## Development

### With Claude Code

DevDock ships with Claude Code skills for agentic development:

| Skill | What it does |
|-------|-------------|
| `/devdock-start` | Start a profile, validate, check health |
| `/devdock-debug` | Diagnose failing services via Tilt API + logs |
| `/devdock-plugin-create` | Scaffold a plugin targeting a lifecycle hook |
| `/devdock-profile-create` | Create a profile interactively |

The MCP server (`devdock mcp serve`) provides 12 tools for AI agents. See `.mcp.json` for configuration.

### Manual

```bash
cargo build                  # Build all crates
cargo test --workspace       # Run tests (132 tests)
cargo clippy --workspace     # Lint (0 warnings required)
cargo fmt --check            # Format check
cargo install --path crates/devdock-cli --debug --force  # Install dev build
```

### Building Plugins

```bash
cd plugins/telemetry/bigquery-sync && cargo build
```

## Deployment

### OSS Binary (public)

The devdock binary has **zero cloud-specific code**. It contains:
- Core CLI commands + lifecycle state machine
- MCP server for AI agent integration
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

### Organization-Specific Plugins

Cloud integrations ship as separate plugin binaries:

```bash
# Install devdock (same binary for everyone)
cargo install devdock

# Install organization plugins
cargo install --git https://github.com/your-org/devdock-plugins devdock-plugin-bigquery-sync
cargo install --git https://github.com/your-org/devdock-plugins devdock-plugin-config-fetcher
```

The plugin binaries go to `~/.cargo/bin/` (in PATH). DevDock finds them by name.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) for development setup, code style, and how to add new backends.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
