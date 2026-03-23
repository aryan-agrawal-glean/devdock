# /devdock-profile-create

Create a new DevDock profile interactively. Discovers available services
from devdock_config.json files, asks which to include, resolves dependencies,
and writes the profile YAML.

## Steps

1. Discover available services:
   - Read `devdock/services/*/devdock_config.json` files
   - List all available services with their dependencies

2. Ask the user:
   - Profile name (e.g., `frontend-only`, `api-dev`)
   - Which services to include (show discovered services)
   - Which infra components to include (sql, redis, opensearch, etc.)

3. Resolve dependencies:
   - Use DFS resolver to find transitive dependencies
   - Show the full resolved set to the user

4. Generate profile YAML at `.devdock/profiles/<name>.yaml`:
   ```yaml
   project: <from devdock.toml>
   services:
     service-name:
       type: local
       version: latest
   infra:
     sql: {}
     redis: {}
   ```

5. Validate the new profile:
   ```
   devdock_validate(profile_name: "<name>")
   ```

6. Optionally dry-run to verify:
   ```bash
   devdock start <name> --dry-run
   ```

## Example profiles

```yaml
# Minimal frontend development
project: my-app
services:
  frontend:
    type: local
  api:
    type: remote
    version: latest
infra:
  redis: {}
```

```yaml
# Full stack local development
project: my-app
services:
  frontend:
    type: local
  api:
    type: local
  worker:
    type: local
infra:
  sql: {}
  redis: {}
  opensearch: {}
```
