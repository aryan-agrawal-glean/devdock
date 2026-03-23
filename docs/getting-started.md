# Getting Started with DevDock

DevDock spins up complex multi-service development environments in under 30 seconds.
It works with any monorepo — Bazel, npm, Cargo, Maven, or custom build systems.

## Quick Start (5 minutes)

```bash
# 1. Install DevDock
curl -fsSL https://raw.githubusercontent.com/aryan-agrawal-glean/devdock/main/install.sh | bash

# 2. Initialize in your repo
cd my-monorepo
devdock init

# 3. Start your environment
devdock start default
```

That's it. DevDock generates the Tiltfile, starts your services, and opens the Tilt UI at
http://localhost:10350.

---

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) — container runtime (required)
- [Tilt](https://docs.tilt.dev/install.html) — orchestration UI (required)
- [Bazel](https://bazel.build/) — only if your repo uses Bazel builds

## Installation

### Binary (recommended)

```bash
curl -fsSL https://raw.githubusercontent.com/aryan-agrawal-glean/devdock/main/install.sh | bash
```

Installs to `/usr/local/bin/devdock`. Supports macOS (ARM64, x86_64) and Linux (x86_64).

### Build from source

```bash
git clone https://github.com/aryan-agrawal-glean/devdock.git
cd devdock
cargo install --path crates/devdock-cli
```

Requires Rust 1.74+. Get it at [rustup.rs](https://rustup.rs/).

### Verify

```bash
devdock --version
devdock --help
```

---

## Setting Up Your Repo

### Step 1: Initialize

```bash
cd my-monorepo
devdock init
```

This creates `.devdock/devdock.toml`. Edit it to match your project:

```toml
[project]
name = "my-project"
domain = "localhost"

[cloud]
platform = "none"    # "none" | "google" | "aws" | "azure"
```

### Step 2: Add Service Definitions

For each service, create a `devdock_config.json` in your service's devdock config directory:

```
devdock/services/
├── api/
│   └── devdock_config.json
└── worker/
    └── devdock_config.json
```

**Bazel project:**
```json
{
  "type": "python",
  "bazel_target": "//services/api:api",
  "binary_mount": "services/api",
  "port": "8080",
  "dependencies": ["sql"],
  "env": {
    "PORT": "8080"
  }
}
```

**npm / non-Bazel project:**
```json
{
  "type": "node",
  "build_command": ["npm", "run", "build"],
  "build_output": "dist/server.js",
  "binary_mount": "/app/server.js",
  "port": "3000",
  "watch_dirs": ["src/", "package.json"]
}
```

**All `devdock_config.json` fields:**

| Field | Required | Description |
|-------|----------|-------------|
| `type` | No | `python`, `go`, `java`, `node`, `rust`, `custom` (default: `custom`) |
| `bazel_target` | No | Bazel build target (e.g. `//services/api:api`) |
| `build_command` | No | Arbitrary build command (e.g. `["npm", "run", "build"]`) |
| `build_output` | No | Path to build artifact (required when using `build_command`) |
| `binary_mount` | No | Where the built artifact is mounted in the container |
| `port` | No | Service port |
| `dependencies` | No | Other services or infra this service depends on |
| `env` | No | Environment variables injected into the container |
| `healthcheck` | No | Docker healthcheck config |
| `watch_dirs` | No | Directories to watch for hot-reload |

> **Note:** If neither `bazel_target` nor `build_command` is set, the service runs as a
> pure Docker service (no local build step). Useful for pre-built images or remote services.

### Step 3: Create a Profile

Profiles declare which services and infra to run:

```bash
mkdir -p .devdock/profiles
```

`.devdock/profiles/default.yaml`:
```yaml
project: my-project
infra:
  sql:
    type: local
services:
  api:
    type: local
```

**Service types:**
- `local` — builds on host, runs in Docker container
- `remote` — kubectl port-forward to a remote service (for transitive deps you don't want to run locally)

### Step 4: Start

```bash
devdock start default
```

DevDock will:
1. Resolve service dependencies (api → sql)
2. Generate runtime configs + Tiltfile
3. Build your services (Bazel or custom build command)
4. Start infra in Docker (MySQL, Redis, etc.)
5. Start your services in Docker
6. Open Tilt UI at http://localhost:10350

### Step 5: Manage

```bash
devdock status             # See running services and health
devdock stop               # Stop everything cleanly
devdock cleanup            # Full cleanup (removes containers, files, state)
devdock validate default   # Validate profile without starting
devdock start default --dry-run  # Preview what would start
```

---

## Infrastructure

DevDock ships default Docker Compose configs for common infra. Add them to your profile:

```yaml
infra:
  sql:         { type: local }    # MySQL 8
  redis:       { type: local }    # Redis 7
  opensearch:  { type: local }    # OpenSearch
  memcached:   { type: local }    # Memcached
  pubsub:      { type: local }    # Google PubSub emulator
  nginx:       { type: local }    # Nginx reverse proxy
```

To override with a custom compose file, add it to your repo:
```bash
mkdir -p devdock/config/infra/sql
# Add docker-compose.yaml with your custom MySQL config
```

---

## Multiple Profiles

Create profiles for different workflows:

```yaml
# .devdock/profiles/minimal.yaml — fast, one service
services:
  api:
    type: local

# .devdock/profiles/full.yaml — everything running
infra:
  sql:       { type: local }
  redis:     { type: local }
  opensearch: { type: local }
services:
  api:     { type: local }
  worker:  { type: local }
  frontend: { type: local }
```

```bash
devdock start minimal   # <30s startup, one service
devdock start full      # Full environment, all services
```

---

## Remote Development

Run services on a remote machine (SSH or Coder workspace) instead of locally:

```bash
devdock start default -m remote
```

This syncs your code via rsync, starts services remotely, and port-forwards them to
`localhost`. Configure in `devdock.toml`:

```toml
[remote]
provider = "ssh"          # "ssh" | "coder"
host = "dev.example.com"  # for SSH
template = "dev"          # for Coder
```

---

## Claude Code Integration (Agent-First)

DevDock ships Claude Code skills that let you manage your environment conversationally.
The `.mcp.json` in the root configures the DevDock MCP server automatically.

Once DevDock is running, open this repo in Claude Code and try:

```
/devdock-start         # Start a profile interactively
/devdock-debug         # Diagnose a failing service
/devdock-profile-create  # Create a new profile for your workflow
/devdock-plugin-create   # Scaffold a new plugin
```

Or use natural language:
- "Start the API service and check if it's healthy"
- "Why is my worker service failing to start?"
- "Create a profile for frontend development only"

The MCP server exposes 12 tools to the AI agent: `devdock_start`, `devdock_stop`,
`devdock_status`, `devdock_logs`, `devdock_validate`, and more.

---

## Plugins

DevDock uses a plugin system for org-specific integrations. Plugins are executables
that respond to lifecycle hooks (e.g. `after_ready`, `on_error`).

Install community plugins:
```bash
devdock plugin list               # Show installed plugins
devdock plugin install <name>     # Install from registry
devdock plugin remove <name>      # Uninstall
```

Create your own:
```bash
devdock plugin create my-notifier --type notifications --language python
```

Configure in `devdock.toml`:
```toml
[plugins.hooks]
after_ready = ["my-notifier"]
on_error    = ["my-notifier"]
```

See [docs/plugins.md](plugins.md) for the full plugin development guide.

---

## Bug Reports

Generate a diagnostic bundle (system info, logs, Tilt status):

```bash
devdock report
```

For Jira integration, configure in `devdock.toml`:
```toml
[diagnostics]
reporter = "jira"
jira_url     = "https://your-org.atlassian.net"
jira_email   = "bot@your-org.com"
jira_project = "ENG"
```

---

## Next Steps

- [Architecture](architecture.md) — how DevDock works internally
- [Plugins](plugins.md) — building custom integrations
- [CONTRIBUTING.md](../CONTRIBUTING.md) — contributing to DevDock
- [CLAUDE.md](../CLAUDE.md) — working on DevDock with Claude Code
