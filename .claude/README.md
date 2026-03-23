# DevDock — Claude Code Integration

This repo is agent-first: Claude Code ships with built-in skills and an MCP server
so you can develop, debug, and extend DevDock conversationally.

## Quick Start

Open this repo in Claude Code. The MCP server auto-configures via `.mcp.json`.

```
/build-and-test      Build workspace and run all tests
/investigate-issue   Diagnose a bug in DevDock
/add-hook            Add a new lifecycle hook
/devdock-e2e         End-to-end test a feature from build → Tilt startup
/test-profile        Test a profile against a fixture repo
/check-tilt-status   Live Tilt resource health check
```

## User-Facing Skills (for people using DevDock)

Located in `skills/` — slash commands for DevDock users:

| Skill | Purpose |
|-------|---------|
| `/devdock-start` | Start a profile interactively |
| `/devdock-debug` | Diagnose a failing service |
| `/devdock-profile-create` | Create a profile for your workflow |
| `/devdock-plugin-create` | Scaffold a new plugin |

## Contributor Skills (for people working ON DevDock)

Located in `.claude/skills/` — slash commands for DevDock contributors:

| Skill | Purpose |
|-------|---------|
| `/build-and-test` | Build workspace, run tests, clippy, fmt, install dev binary |
| `/investigate-issue` | Trace a bug through the lifecycle state machine |
| `/add-hook` | Add a new lifecycle hook to the state machine |
| `/check-tilt-status` | Live Tilt resource health check |
| `/devdock-e2e` | End-to-end test a feature (build → plugin → start → Tilt verify) |
| `/test-profile` | Test a profile against a fixture repo |

## MCP Tools

When DevDock is running, Claude Code has access to these tools via the MCP server:

```
devdock_start, devdock_stop, devdock_status, devdock_logs,
devdock_validate, devdock_generate, devdock_list_profiles,
devdock_list_services, devdock_plugin_create, devdock_exec,
devdock_cleanup, devdock_report
```

## Architecture

See docs/architecture.md for the full system design.
See CLAUDE.md for build commands and conventions.
