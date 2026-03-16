# /devdock-plugin-create

Scaffold a new DevDock plugin. Asks which lifecycle hook the plugin targets,
generates the boilerplate, and updates devdock.toml with the hook registration.

## Steps

1. Ask the user:
   - Plugin name (e.g., `my-cache-warmer`)
   - Target lifecycle hook (see hook reference below)
   - Language: `rust` (recommended), `python`, or `bash`

2. Scaffold the plugin:
   ```
   devdock_plugin_create(name: "<name>", hook: "<hook>", language: "<lang>")
   ```
   Or manually create the files in `plugins/<category>/<name>/`

3. Generate the main file with correct `handle()` match arms for the chosen hook

4. Update `.devdock/devdock.toml` to register the plugin:
   ```toml
   [plugins.hooks]
   <hook> = ["devdock-plugin-<name>"]
   ```

5. For resource hooks, add to resource_hooks section:
   ```toml
   [plugins.resource_hooks.<resource>]
   after_start = ["devdock-plugin-<name>"]
   ```

## Hook Reference

| Hook | When | Category |
|------|------|----------|
| `after_profile_loaded` | Profile YAML loaded | config |
| `before_infra_ready` | Before Docker/ports | auth |
| `after_infra_ready` | After Docker/ports | infra |
| `before_config_generated` | Before config files | config |
| `after_config_generated` | After config files | config |
| `before_tilt` | Before Tilt spawns | config |
| `after_tilt` | After Tilt is up | telemetry |
| `after_ready` | All resources healthy | notifications |
| `before_stop` | Ctrl+C / exit signal | telemetry |
| `after_stop` | After cleanup | diagnostics |
| `on_error` | Unrecoverable error | diagnostics |
| `on_report` | `devdock report` command | diagnostics |

## Plugin Categories → Directories

- `auth` → `plugins/auth/`
- `config` → `plugins/config/`
- `infra` → `plugins/infra/`
- `telemetry` → `plugins/telemetry/`
- `notifications` → `plugins/notifications/`
- `diagnostics` → `plugins/diagnostics/`
