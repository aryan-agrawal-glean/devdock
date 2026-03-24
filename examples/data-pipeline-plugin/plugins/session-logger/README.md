# Plugin: session-logger

A minimal DevDock lifecycle plugin that logs session telemetry to stderr.

## What It Does

| Hook | Action |
|------|--------|
| `after_tilt` | Logs session start (session_id, profile name, timestamp) |
| `before_stop` | Logs session end timestamp |
| `on_error` | Logs error message and session context |

## Protocol

DevDock plugins communicate via JSON-lines on stdin/stdout:

```
→ {"method": "initialize", "params": {}}
← {"status": "ok", "name": "session-logger", "version": "0.1.0"}

→ {"method": "after_tilt", "params": {"session_id": "abc123", "profile": "default"}}
← {"status": "ok"}

→ {"method": "shutdown", "params": {}}
← {"status": "ok"}
```

## Testing

```bash
chmod +x main.py

echo '{"method":"initialize","params":{}}
{"method":"after_tilt","params":{"session_id":"test-123","profile":"default"}}
{"method":"before_stop","params":{"session_id":"test-123"}}
{"method":"shutdown","params":{}}' | ./main.py
```

Expected output (to stdout — the response stream):
```json
{"status": "ok", "name": "session-logger", "version": "0.1.0"}
{"status": "ok"}
{"status": "ok"}
{"status": "ok"}
```

Expected stderr (human-readable logs):
```
[session-logger] Session started | id=test-123 profile=default at=2024-01-15T10:30:00
[session-logger] Session ending  | id=test-123 at=2024-01-15T10:45:00
```

## Extending

Replace the `print(... file=sys.stderr)` calls with your actual telemetry backend:
- **BigQuery**: use `google-cloud-bigquery` to insert a row per session
- **Datadog**: use `datadog-api-client` to send metrics
- **Slack**: use `slack-sdk` to post a message when sessions end or error
- **File log**: write to `/var/log/devdock-sessions.jsonl` for local auditing

For a production telemetry plugin example, see `plugins/telemetry/bigquery-sync/` in the DevDock repo.

## Requirements

Python 3.8+ (no extra dependencies — uses stdlib only).
