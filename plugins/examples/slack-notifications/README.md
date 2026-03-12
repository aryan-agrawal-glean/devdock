# Slack Notifications Plugin

Sends DevDock error and session notifications to Slack.

## Configuration

```toml
# devdock.toml
[diagnostics]
reporter = "external"
plugin = "~/.devdock/plugins/slack-notifications/plugin.py"
webhook_url = "https://hooks.slack.com/services/YOUR/WEBHOOK/URL"
```

## Notifications

- Session end (success/failure with duration)
- Errors with message
