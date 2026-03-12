# Prometheus Telemetry Plugin

Pushes DevDock telemetry metrics to a Prometheus push gateway.

## Setup

```bash
pip install prometheus-client
```

## Configuration

```toml
# devdock.toml
[telemetry]
backend = "external"
plugin = "~/.devdock/plugins/prometheus-telemetry/plugin.py"
push_gateway = "localhost:9091"
```

## Metrics

- `devdock_sessions_total` — Counter of total sessions started
- `devdock_bootstrap_ms` — Gauge of last bootstrap time in milliseconds
