# DevDock Development Guide

## Project Overview

DevDock is an open-source CLI for managing local and remote development environments.
It works with any Bazel-managed monorepo. The binary generates Tiltfiles at runtime,
ships Starlark libraries for service orchestration, and uses external plugins for
cloud integrations (telemetry, storage, diagnostics).

## Architecture

```
devdock start <profile>
├─ Lifecycle state machine (devdock-lifecycle crate)
│  Initializing → ProfileLoaded → PluginsReady → InfraReady
│  → ConfigReady → TiltfileReady → TiltRunning
│  → ResourcesStarting → Running → Stopping → Stopped
├─ 14 plugin hooks fire at each state transition
├─ Per-resource hooks (before/after_resource_start)
├─ Generates runtime configs + Tiltfile
├─ Extracts shipped libs to devdock/runtime_generated/core/lib/
└─ Starts Tilt
```

## Key Directories

- `crates/` — Rust source code (12 crates, zero cloud-specific code)
- `lib/` — Shipped Starlark libraries (embedded via include_str!)
- `infra/` — Default infra compose files
- `plugins/` — Plugin SDK + phase-based plugins (telemetry/, notifications/, diagnostics/)
- `skills/` — Claude Code skills (devdock-start, devdock-debug, etc.)
- `tests/fixtures/` — Test repos (minimal-python, multi-service, with-infra)

## Build & Test

```bash
cargo build                     # Build all crates
cargo test --workspace          # Run all tests (141 tests)
cargo clippy --workspace        # Lint (must be 0 warnings)
cargo fmt --check               # Format check

# Build a specific plugin
cd plugins/telemetry/bigquery-sync && cargo build

# Install dev binary
cargo install --path crates/devdock-cli --debug --force
```

## Testing Against Your Monorepo

```bash
cd ~/workspace/your-monorepo
devdock start <profile>         # Start a profile
# Check Tilt UI at http://localhost:10350
# Verify all resources are green

# Check resource status programmatically:
curl -s http://localhost:10350/api/view | python3 -c "
import json, sys
data = json.load(sys.stdin)
for r in data.get('uiResources', []):
    s = r.get('status', {})
    name = r.get('metadata', {}).get('name', '?')
    rt = s.get('runtimeStatus', '?')
    if rt in ('ok', 'error', 'pending'):
        print(f'{name:30s} {rt}')
"
```

## Profile Testing Checklist

For each profile:
1. `devdock start <profile>` — starts successfully
2. Tilt UI loads at http://localhost:10350
3. All infra resources show `runtime=ok`
4. All local service builds complete (`update=ok`)
5. All local service containers healthy (`runtime=ok`)
6. Remote services show under remote-service label
7. `devdock stop` — stops cleanly

## Plugin Development

```bash
# Create a new plugin
devdock plugin create my-plugin --type telemetry --language python

# Test plugin protocol
echo '{"method":"initialize","config":{}}
{"method":"emit","event":{"type":"SessionStart","session_id":"test"}}
{"method":"shutdown"}' | ./my-plugin
```

## Key Files

| File | Purpose |
|------|---------|
| `crates/devdock-core/src/tiltfile_generator.rs` | Generates the Tiltfile |
| `crates/devdock-core/src/project_config.rs` | Parses devdock.toml |
| `crates/devdock-core/src/service_config.rs` | Parses devdock_config.json |
| `crates/devdock-core/src/resolver.rs` | DFS dependency resolver |
| `crates/devdock-cli/src/commands/start.rs` | Main start command |
| `crates/devdock-lifecycle/src/machine.rs` | Lifecycle state machine |
| `crates/devdock-lifecycle/src/hooks.rs` | 14 lifecycle hooks |
| `crates/devdock-mcp/src/tools.rs` | MCP server tools (12 tools) |
| `lib/service.Tiltfile` | Service orchestration (shipped) |
| `lib/docker.Tiltfile` | Dockerfile/compose generation (shipped) |
| `plugins/sdk/src/lib.rs` | Plugin SDK |

## Conventions

- No org-specific code in `crates/` — all cloud/org integrations go in external plugins
- All config comes from `devdock.toml` or env vars, never hardcoded
- Tests use fixture repos in `tests/fixtures/`
- Pre-commit hooks enforce: cargo fmt, clippy, no large files
- Commit messages: `feat:`, `fix:`, `refactor:`, `chore:`, `docs:`, `test:`
