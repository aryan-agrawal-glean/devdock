# Getting Started with DevDock

## Prerequisites

- [Docker](https://docs.docker.com/get-docker/) — container runtime
- [Tilt](https://docs.tilt.dev/install.html) — container orchestration
- [Bazel](https://bazel.build/) — build system (for Bazel-managed repos)
- [Rust 1.74+](https://rustup.rs/) — to build from source

## Installation

```bash
git clone https://github.com/askscio/devdock.git
cd devdock
cargo install --path crates/devdock-cli
```

Verify:
```bash
devdock --version
```

## Setting Up Your Repo

### Step 1: Initialize

```bash
cd my-monorepo
devdock init
```

This creates `.devdock/devdock.toml`. Edit it:

```toml
[project]
name = "my-project"
domain = "localhost"

[cloud]
platform = "none"    # or "google", "aws", "azure"
```

### Step 2: Add Service Definitions

For each service in your monorepo, create a `devdock_config.json`:

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
  "dependencies": ["sql"],
  "env": {
    "PORT": "8080"
  },
  "healthcheck": {
    "test": ["CMD", "curl", "-f", "http://localhost:8080/health"]
  }
}
```

**Fields:**
- `type` — `python`, `go`, `java`, or `custom` (default: `custom`)
- `bazel_target` — Bazel build target. If omitted, service is skipped during build.
- `binary_mount` — where bazel output goes (relative to repo root)
- `dependencies` — other services or infra this service needs
- `port` — service port
- `healthcheck` — Docker healthcheck (optional)
- `requirements` — Python-specific: paths to requirements.txt and wheels

### Step 3: Create a Profile

```bash
mkdir -p devdock/profiles
```

`devdock/profiles/default.yaml`:
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
- `local` — Bazel builds on host, runs in Docker container
- `remote` — kubectl port-forward to remote service (for transitive deps)

### Step 4: Start

```bash
devdock start default
```

DevDock will:
1. Resolve dependencies (api → sql)
2. Generate runtime configs (devdock.ini, temp_profile.yaml, Tiltfile)
3. Build your service with Bazel
4. Start MySQL in Docker
5. Start your service in Docker
6. Open Tilt UI at http://localhost:10350

### Step 5: Check and Stop

```bash
devdock status    # See running services
devdock stop      # Stop everything
devdock cleanup   # Remove containers and files
```

## Adding Infrastructure

DevDock ships default compose files for: MySQL, Redis, OpenSearch, Memcached, PubSub, Nginx.

Just add them to your profile:
```yaml
infra:
  sql:
    type: local
  redis:
    type: local
  opensearch:
    type: local
```

To use custom compose files (e.g., specific MySQL version or flags), add them to your repo:
```bash
mkdir -p devdock/config/infra/sql
# Add docker-compose.yaml with your custom config
```

## Multiple Profiles

Create different profiles for different workflows:

```yaml
# devdock/profiles/minimal.yaml — just the API
services:
  api:
    type: local

# devdock/profiles/full.yaml — everything
infra:
  sql: { type: local }
  redis: { type: local }
  opensearch: { type: local }
services:
  api: { type: local }
  worker: { type: local }
  frontend: { type: local }
```

```bash
devdock start minimal    # Fast startup, single service
devdock start full       # Full environment
```

## Remote Development

Start services on a remote workspace (via SSH or Coder):

```bash
devdock start default -m remote
```

This will:
1. Create/connect to a remote workspace
2. Sync your code via rsync
3. Start services remotely
4. Port-forward services to your local machine

Configure the remote provider in `devdock.toml`:
```toml
[remote]
provider = "ssh"        # or "coder"
template = "my-template"  # for Coder
```

## Bug Reports

Generate a diagnostic report with system info, logs, and Tilt status:

```bash
devdock report
```

Configure Jira integration for automatic ticket creation:
```toml
[diagnostics]
reporter = "jira"
jira_url = "https://your-org.atlassian.net"
jira_email = "bot@your-org.com"
jira_project = "ENG"
secret_project = "your-gcp-project"
```

## External Plugins

Create custom backends for telemetry, diagnostics, or storage:

```bash
devdock plugin create my-datadog --type telemetry --language python
# Edit ~/.devdock/plugins/my-datadog/plugin.py
```

Configure in devdock.toml:
```toml
[telemetry]
backend = "external"
plugin = "~/.devdock/plugins/my-datadog/plugin.py"
```

See [docs/plugins.md](plugins.md) for the full plugin development guide.

## Next Steps

- [Architecture](architecture.md) — system design and data flow
- [Plugins](plugins.md) — plugin development guide
- [CONTRIBUTING.md](../CONTRIBUTING.md) — how to contribute
- [CLAUDE.md](../CLAUDE.md) — development guide for Claude Code
