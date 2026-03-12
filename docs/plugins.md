# DevDock Plugins

DevDock supports external plugins for custom telemetry, diagnostics, and other backends.

## Creating a Plugin

```bash
devdock plugin create my-plugin --type telemetry --language python
```

This creates `~/.devdock/plugins/my-plugin/` with:
- `plugin.py` — Plugin implementation with protocol boilerplate
- `devdock-plugin.toml` — Plugin metadata
- `requirements.txt` — Python dependencies

## Protocol

Plugins communicate via JSON-lines over stdin/stdout:

```
→ {"method":"initialize","config":{"key":"value"}}
← {"status":"ok"}

→ {"method":"emit","event":{"type":"SessionStart","session_id":"abc"}}
← {"status":"ok"}

→ {"method":"shutdown"}
← {"status":"ok"}
```

## Configuration

```toml
# devdock.toml
[telemetry]
backend = "external"
plugin = "~/.devdock/plugins/my-plugin/plugin.py"
# Additional config passed to initialize:
push_gateway = "localhost:9091"
```

## Examples

See `plugins/examples/` in the devdock repo:
- `prometheus-telemetry` — Push metrics to Prometheus
- `slack-notifications` — Send Slack alerts on errors

## Supported Languages

Plugin templates available for:
- **Python** — `--language python`
- **Go** — `--language go`
- **Bash** — `--language bash`

Any language that can read stdin and write stdout works.
