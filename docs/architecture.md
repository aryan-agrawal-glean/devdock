# DevDock Architecture

## Overview

DevDock is a Rust CLI that generates runtime configuration and Tiltfiles for managing
development environments. It uses Tilt for container orchestration and external plugins
for cloud integrations.

## Pipeline

```
devdock start <profile>
│
├─[1] Load .devdock/devdock.toml
│     → Project name, services_dir, infra_dir, cloud config, diagnostics
│
├─[2] Load profile YAML
│     → Which services (local/remote), which infra, env overrides
│
├─[3] Resolve dependencies (DFS)
│     → Reads devdock_config.json per service
│     → Discovers transitive deps (api → sql, redis)
│     → Separates services from infra
│
├─[4] Docker health check + network setup
│     → devdock_network, devdock_infra
│
├─[5] Instance registration + port allocation
│     → SQLite state DB at ~/.devdock/devdock.db
│
├─[6] Set environment variables
│     → DEVDOCK_ID, CLOUD_PLATFORM, DEVDOCK_ROOT
│     → DEVDOCK_JIRA_URL, DEVDOCK_SLACK_CHANNEL (from diagnostics config)
│
├─[7] Parallel config generation
│     │
│     ├─[7a] devdock.ini
│     │   → Service configuration (SQL host, Redis port, etc.)
│     │   → Platform-specific settings
│     │   → INI overrides from profile
│     │
│     ├─[7b] temp_profile.yaml
│     │   → Enriched profile with env vars, platform info
│     │   → Consumed by Tiltfile at runtime
│     │
│     ├─[7c] watch_file_*.yaml
│     │   → Per-service file watch patterns
│     │   → Static glob patterns (no Bazel query required)
│     │
│     └─[7d] Tiltfile
│         → Generated orchestrator
│         → Loads shipped Starlark libs
│         → For each local service: setup_service()
│         → For each remote service: setup_remote_service()
│         → For each infra: load Tiltfile or docker_compose
│
├─[8] Extract shipped Starlark libs
│     → Copies lib/*.Tiltfile to devdock/core/lib/
│     → Copies templates/*.tpl to devdock/core/templates/
│
├─[9] Start Tilt
│     → tilt up from devdock/runtime_generated/
│     → Tilt loads generated Tiltfile
│     │
│     └─ For each local service:
│        ├─ Bazel build on host → ~/.devdock/{service}/
│        ├─ Generate Dockerfile from template
│        ├─ Generate docker-compose.yaml
│        ├─ Docker build + start container
│        └─ Volume mount binary from ~/.devdock/{service}/
│
└─[10] Monitor + telemetry
      → Startup monitor tracks resource health
      → Telemetry collected to file (sync via external plugin)
```

## Crate Architecture

```
devdock-cli (binary)
├── commands/start.rs         → Main pipeline orchestration
├── commands/init.rs          → Project initialization
├── commands/report.rs        → Bug reporting
├── commands/remote.rs        → Remote workspace management
└── commands/monitor.rs       → Health monitoring

devdock-core (library)
├── project_config.rs         → devdock.toml parser
├── service_config.rs         → devdock_config.json parser
├── resolver.rs               → DFS dependency resolver
├── tiltfile_generator.rs     → Tiltfile generation + lib extraction
├── plugin/                   → Plugin trait interfaces
│   ├── traits.rs             → Backend traits (telemetry, storage, etc.)
│   ├── registry.rs           → Backend registry
│   └── external.rs           → External plugin protocol
└── profile.rs                → Profile YAML types

devdock-config (library)
├── generator.rs              → Config generation orchestrator
├── ini_generator.rs          → devdock.ini generation
├── profile_generator.rs      → temp_profile.yaml generation
├── platform.rs               → GCP/AWS/Azure auth
├── dependency_graph.rs       → Service dependency graph
└── watch_patterns.rs         → File watch pattern generation

devdock-remote (library)
├── coder.rs                  → Coder workspace management
├── workspace.rs              → Remote startup orchestration
├── config_sync.rs            → Config sync to remote
├── ssh.rs                    → SSH utilities
└── port_forward.rs           → Port forwarding

devdock-telemetry (library)
├── collector.rs              → Telemetry collection
├── emitter.rs                → FileEmitter (built-in)
└── models.rs                 → TelemetrySnapshot data model

devdock-diagnostics (library)
├── report.rs                 → Bug report workflow
├── jira.rs                   → Jira ticket creation (config-driven)
├── slack.rs                  → Slack notifications (config-driven)
└── system.rs                 → System info collection
```

## Shipped Starlark Libraries

These are embedded in the binary and extracted at runtime:

```
lib/
├── service.Tiltfile          → setup_service(name, config)
│   ├── Reads devdock_config.json
│   ├── Bazel build + rsync to ~/.devdock/{service}/
│   ├── Generates Dockerfile from template
│   ├── Generates docker-compose.yaml
│   └── Creates dc_resource with dependencies
│
├── docker.Tiltfile           → setup_docker_build(context)
│   ├── Generates Dockerfile from template
│   ├── Generates docker-compose.yaml
│   └── Handles volume mounts for binary + config
│
├── bazel.Tiltfile            → bazel_build(name, target, ...)
│   ├── Runs bazel build on host
│   ├── Rsyncs output to ~/.devdock/{service}/
│   └── Sets up file watching via watch_file_*.yaml
│
├── remote_service.Tiltfile   → setup_remote_service(name, port)
│   └── kubectl port-forward to remote service
│
├── central_bazel.Tiltfile    → setup_central_bazel_build(profile)
│   └── Parallel Bazel compilation for non-Go services
│
└── templates/
    ├── Dockerfile.python.tpl → Python service Dockerfile
    ├── Dockerfile.go.tpl     → Go service Dockerfile
    ├── Dockerfile.tpl        → Java/generic Dockerfile
    └── compose.yaml.tpl      → docker-compose template
```

## Plugin Architecture

```
┌─────────────────────────────────────────────────────────────┐
│                    devdock binary                            │
│                                                              │
│  ┌──────────────────────────────────────────────────────┐   │
│  │ Built-in (always available)                          │   │
│  │  • FileEmitter (telemetry to local file)             │   │
│  │  • LocalStorage (config from filesystem)             │   │
│  │  • SSH (remote dev)                                  │   │
│  │  • Stdout (diagnostics)                              │   │
│  └──────────────────────────────────────────────────────┘   │
│                         │                                    │
│                   JSON stdin/stdout                          │
│                         │                                    │
│  ┌──────────────────────▼──────────────────────────────┐   │
│  │ External Plugins (separate executables)              │   │
│  │                                                      │   │
│  │  Common (plugins/common/):                           │   │
│  │  • bigquery-sync    → telemetry to BigQuery          │   │
│  │  • jira-reporter    → bug reports to any Jira        │   │
│  │  • slack-notifier   → notifications to any Slack     │   │
│  │                                                      │   │
│  │  Glean (plugins/glean/):                             │   │
│  │  • gcs-storage      → config from GCS buckets        │   │
│  │  • s3-storage       → config from S3 buckets         │   │
│  │                                                      │   │
│  │  Custom (devdock plugin create):                     │   │
│  │  • Any language (Python, Go, Bash, Rust)             │   │
│  └──────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────┘
```

### Plugin Protocol

Every external plugin is an executable that reads JSON from stdin and writes JSON to stdout:

```json
→ {"method":"initialize","config":{"project":"my-project"}}
← {"status":"ok"}

→ {"method":"emit","event":{"type":"SessionStart","session_id":"abc"}}
← {"status":"ok"}

→ {"method":"shutdown"}
← {"status":"ok"}
```

The Plugin SDK (`plugins/sdk/`) handles protocol boilerplate. Plugin authors just implement `handle()`.

## Data Flow

```
devdock_config.json (per service)
  ↓
Dependency Resolver (DFS)
  ↓
┌─────────────────────────────────┐
│ Parallel Generation             │
│ ├── devdock.ini     (services)  │
│ ├── temp_profile.yaml (Tilt)    │
│ ├── watch_file_*.yaml (reload)  │
│ └── Tiltfile         (orchestr) │
└────────────────┬────────────────┘
                 ↓
Tilt reads temp_profile.yaml + Tiltfile
  ↓
For each service:
  ├── Bazel build on host → ~/.devdock/{service}/
  ├── Dockerfile from template
  ├── docker-compose.yaml from template
  ├── Docker build
  └── Container starts with volume mounts:
      ├── ~/.devdock/{service}/ → /app/{service}/     (binary)
      ├── config/devdock.ini   → /config/devdock.ini  (config)
      └── ~/.config/gcloud/    → /tmp/.config/gcloud/ (credentials)
```

## State Management

DevDock uses SQLite at `~/.devdock/devdock.db` to track:
- Running instances (ID, profile, branch, ports, status)
- Port allocations (prevent conflicts)
- Sync sessions (for remote dev)

## Remote Development

```
Local Machine                    Remote Workspace (Coder/SSH)
┌──────────────┐                ┌──────────────────┐
│ devdock CLI  │    SSH/Coder   │ Docker daemon    │
│ ├─ Config gen│───────────────→│ ├─ Tilt          │
│ ├─ File sync │    rsync      │ ├─ Services      │
│ │  (daemon)  │───────────────→│ └─ Infra         │
│ └─ Port fwd  │    SSH tunnel │                    │
│              │←──────────────│                    │
└──────────────┘                └──────────────────┘
```
