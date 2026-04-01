# DevDock Plugins

DevDock uses external plugins for telemetry, notifications, diagnostics, and cloud integrations. Plugins are standalone executables that communicate via JSON-lines over stdin/stdout.

## Lifecycle Hooks

Plugins are registered to specific lifecycle hooks in `devdock.toml`. DevDock fires 14 hooks at state transitions:

| Hook | When | Example Use |
|------|------|-------------|
| `after_profile_loaded` | Profile YAML loaded | Validate custom fields |
| `before_infra_ready` | Before Docker/ports | Cloud auth, credentials |
| `after_infra_ready` | After Docker/ports | Custom network setup |
| `before_config_generated` | Before config files | Fetch remote configs (GCS/S3) |
| `after_config_generated` | After config files | Write devdock.ini, enrich configs |
| `before_tilt` | Before Tilt spawns | Last-minute config enrichment |
| `after_tilt` | After Tilt is up | Telemetry: session started |
| `before_resource_start` | Before resource starts | Per-resource setup |
| `after_resource_start` | After resource healthy | SQL hydration, search reindex |
| `after_ready` | All resources healthy | Slack notification |
| `before_stop` | Ctrl+C / exit signal | Final telemetry flush |
| `after_stop` | After cleanup | Post-cleanup actions |
| `on_error` | Unrecoverable error | Create Jira ticket |
| `on_report` | `devdock report` command | Bug report submission |

## Configuration

```toml
# devdock.toml

[plugins.hooks]
before_infra_ready = ["devdock-plugin-platform-auth"]
after_config_generated = ["devdock-plugin-config-writer"]
after_tilt = ["devdock-plugin-bigquery-sync"]
after_ready = ["devdock-plugin-slack-notifier"]
before_stop = ["devdock-plugin-bigquery-sync"]
on_error = ["devdock-plugin-jira-reporter", "devdock-plugin-slack-notifier"]
on_report = ["devdock-plugin-jira-reporter"]

# Per-resource hooks
[plugins.resource_hooks.sql]
after_start = ["devdock-plugin-sql-hydrator"]

[plugins.resource_hooks.opensearch]
after_start = ["devdock-plugin-opensearch-hydrator"]
```

## Protocol

Plugins communicate via JSON-lines over stdin/stdout:

```
→ {"method":"initialize","config":{"project":"my-project","platform":"google"}}
← {"status":"ok"}

→ {"method":"after_tilt","params":{"session_id":"dev-abc","profile_name":"default"}}
← {"status":"ok"}

→ {"method":"after_resource_start","params":{"resource_name":"sql","resource_kind":"infra"}}
← {"status":"ok"}

→ {"method":"shutdown"}
← {"status":"ok"}
```

## Creating a Plugin

### From template

```bash
devdock plugin create my-plugin --type telemetry --language python
```

This creates `~/.devdock/plugins/my-plugin/` with boilerplate for the chosen language.

### With Rust SDK

```rust
use devdock_plugin_sdk::{ok, Plugin};
use serde_json::Value;

#[derive(Default)]
struct MyPlugin;

impl Plugin for MyPlugin {
    fn name(&self) -> &str { "my-plugin" }

    fn initialize(&mut self, config: Value) {
        // Setup from devdock.toml config
    }

    fn handle(&self, method: &str, params: &Value) -> Value {
        match method {
            "after_ready" => {
                let profile = params["profile_name"].as_str().unwrap_or("?");
                let count = params["resource_count"].as_u64().unwrap_or(0);
                eprintln!("All {count} resources ready for profile {profile}!");
                ok()
            }
            "on_error" => {
                let error = params["error"].as_str().unwrap_or("unknown");
                eprintln!("Error: {error}");
                ok()
            }
            _ => ok(), // Ignore unhandled hooks
        }
    }
}

fn main() {
    devdock_plugin_sdk::run::<MyPlugin>();
}
```

Add to `Cargo.toml`:
```toml
[dependencies]
devdock-plugin-sdk = { path = "path/to/plugins/sdk" }
serde_json = "1.0"
```

## Supported Languages

Plugin templates available for:
- **Rust** — `--language rust` (uses `devdock-plugin-sdk` crate)
- **Python** — `--language python`
- **Go** — `--language go`
- **Bash** — `--language bash`

Any language that can read stdin and write stdout works.

## Examples

See `plugins/examples/` in the devdock repo:
- `prometheus-telemetry` — Push metrics to Prometheus
- `slack-notifications` — Send Slack alerts on errors

## Plugin Directory Layout

```
plugins/
├── sdk/                        # Plugin SDK (Rust crate)
├── telemetry/
│   └── bigquery-sync/          # after_tilt, after_ready, before_stop
├── notifications/
│   └── slack-notifier/         # after_ready, on_error
├── diagnostics/
│   └── jira-reporter/          # on_error, on_report
├── templates/                  # Plugin scaffolding
└── examples/                   # Reference implementations
```

Organization-specific plugins live in separate repos. Build them using the Rust SDK or any language, publish as binaries, and reference them in your `devdock.toml`.
