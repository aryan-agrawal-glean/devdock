# DevDock Architecture

## Overview

DevDock is a Rust CLI that uses a **lifecycle state machine** to orchestrate development
environments. It generates runtime configuration and Tiltfiles, manages plugin hooks at
each state transition, and supports AI agent integration via an MCP server.

## Lifecycle State Machine

The startup pipeline is driven by `devdock-lifecycle::LifecycleMachine`, which transitions
through 12 states. Plugin hooks fire at each transition:

```
 Initializing ─── Parse CLI args, find workspace root
      │
      ▼
 ProfileLoaded ── Load profile YAML, validate, resolve deps
      │            hook: after_profile_loaded
      ▼
 PluginsReady ─── Load devdock.toml, spawn plugin processes
      │            hook: before_infra_ready (auth, credentials)
      ▼
 InfraReady ───── Docker check, networks, port allocation, instance registration
      │            hook: before_config_generated (fetch remote configs)
      ▼
 ConfigReady ──── Generate temp_profile.yaml (+ INI via plugin)
      │            hook: after_config_generated (write devdock.ini)
      ▼
 TiltfileReady ── Discover services, resolve deps, generate Tiltfile
      │            hook: before_tilt (last-minute config)
      ▼
 TiltRunning ──── Spawn `tilt up`, extract Starlark libs
      │            hook: after_tilt (telemetry: session started)
      ▼
 ResourcesStarting ── Per-resource loop:
      │   for each resource:
      │     hook: before_resource_start(name, kind)
      │     [Tilt starts container, wait healthy]
      │     hook: after_resource_start(name, kind)
      ▼
 Running ──────── All resources healthy, steady state
      │            hook: after_ready (notifications)
      ▼
 Stopping ─────── Ctrl+C or Tilt exits
      │            hook: before_stop (final telemetry)
      ▼
 Stopped ──────── Cleanup complete
                   hook: after_stop

 Any state → Failed → hook: on_error → Stopping
```

## Hook Reference

| Hook | When | Params |
|------|------|--------|
| `after_profile_loaded` | Profile YAML loaded | `profile_name`, `workspace_root`, `platform` |
| `before_infra_ready` | Before Docker/ports | `profile_name`, `instance_id`, `port_base` |
| `after_infra_ready` | After Docker/ports | same as above |
| `before_config_generated` | Before config files | `profile_name`, `runtime_dir`, `workspace_root` |
| `after_config_generated` | After config files | same as above |
| `before_tilt` | Before Tilt spawns | `session_id`, `profile_name` |
| `after_tilt` | After Tilt is up | same as above |
| `before_resource_start` | Before resource starts | `resource_name`, `resource_kind` |
| `after_resource_start` | After resource healthy | same as above |
| `after_ready` | All resources healthy | `session_id`, `resource_count`, `startup_duration_secs` |
| `before_stop` | Ctrl+C / exit signal | `session_id`, `duration_secs` |
| `after_stop` | After cleanup | same as above |
| `on_error` | Unrecoverable error | `session_id`, `error`, `state` |
| `on_report` | `devdock report` | `title`, `body` |

## Crate Architecture

```
devdock-cli (binary — thin orchestrator)
├── commands/start.rs         → Creates LifecycleMachine, calls advance()
├── commands/init.rs          → Project initialization
├── commands/report.rs        → Bug reporting
├── commands/remote.rs        → Remote workspace management
└── commands/monitor.rs       → Health monitoring

devdock-lifecycle (library — state machine)
├── state.rs                  → State enum + transitions
├── machine.rs                → LifecycleMachine (advance, fire hooks, fail)
├── hooks.rs                  → Hook enum + HookRunner (dispatches to PluginManager)
└── context.rs                → Typed context structs (ProfileContext, InfraContext, etc.)

devdock-mcp (library — MCP server)
├── server.rs                 → JSON-RPC stdin/stdout server
└── tools.rs                  → 12 MCP tools (start, stop, status, logs, etc.)

devdock-core (library — foundation)
├── project_config.rs         → devdock.toml parser (PluginHooks, ResourceHooks)
├── service_config.rs         → devdock_config.json parser
├── resolver.rs               → DFS dependency resolver
├── tiltfile_generator.rs     → Tiltfile generation + lib extraction
├── plugin_manager.rs         → Plugin spawn/dispatch/shutdown
├── plugin/                   → Plugin trait interfaces
└── profile.rs                → Profile YAML types

devdock-config (library)
├── loader.rs                 → Profile loading + validation
├── dependency.rs             → Service dependency graph
└── watch_patterns.rs         → File watch pattern generation

devdock-remote (library)
├── coder.rs                  → Coder workspace management
├── workspace.rs              → Remote startup orchestration
└── port_forward.rs           → Port forwarding

devdock-telemetry (library)
├── emitter.rs                → FileEmitter (built-in)
└── models.rs                 → TelemetrySnapshot data model

devdock-diagnostics (library)
├── report.rs                 → Bug report workflow
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
├── bazel.Tiltfile            → bazel_build(name, target, ...)
├── remote_service.Tiltfile   → setup_remote_service(name, port)
├── central_bazel.Tiltfile    → setup_central_bazel_build(profile)
│
└── templates/
    ├── Dockerfile.python.tpl
    ├── Dockerfile.go.tpl
    ├── Dockerfile.tpl
    └── compose.yaml.tpl
```

## Plugin Architecture

```
┌──────────────────────────────────────────────────────────┐
│                     devdock binary                        │
│                                                          │
│  ┌────────────────────────────────────────────────────┐  │
│  │ Built-in (always available)                        │  │
│  │  • FileEmitter (telemetry to local file)           │  │
│  │  • LocalStorage (config from filesystem)           │  │
│  │  • SSH (remote dev)                                │  │
│  │  • Stdout (diagnostics)                            │  │
│  └────────────────────────────────────────────────────┘  │
│                        │                                  │
│                  JSON stdin/stdout                        │
│                        │                                  │
│  ┌─────────────────────▼──────────────────────────────┐  │
│  │ External Plugins (separate executables)             │  │
│  │                                                     │  │
│  │  Telemetry (plugins/telemetry/):                    │  │
│  │  • bigquery-sync    → after_tilt, after_ready       │  │
│  │                                                     │  │
│  │  Notifications (plugins/notifications/):            │  │
│  │  • slack-notifier   → after_ready, on_error         │  │
│  │                                                     │  │
│  │  Diagnostics (plugins/diagnostics/):                │  │
│  │  • jira-reporter    → on_error, on_report           │  │
│  │                                                     │  │
│  │  Glean (plugins/glean/):                            │  │
│  │  • platform-auth    → before_infra_ready            │  │
│  │  • config-fetcher   → before_config_generated       │  │
│  │  • config-writer    → after_config_generated        │  │
│  │  • sql-hydrator     → after_resource_start("sql")   │  │
│  │  • opensearch-hydrator → after_resource_start       │  │
│  └─────────────────────────────────────────────────────┘  │
└──────────────────────────────────────────────────────────┘
```

### Plugin Protocol

Plugins are executables that read JSON-lines from stdin and write JSON-lines to stdout:

```json
→ {"method":"initialize","config":{"project":"my-project","platform":"google"}}
← {"status":"ok"}

→ {"method":"after_tilt","params":{"session_id":"dev-abc","profile_name":"default"}}
← {"status":"ok"}

→ {"method":"after_resource_start","params":{"resource_name":"sql","resource_kind":"infra"}}
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
│ Config Generation               │
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
  └── Container starts with volume mounts
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
