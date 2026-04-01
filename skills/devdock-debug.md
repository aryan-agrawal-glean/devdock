# /devdock-debug

Diagnose a failing DevDock service. Checks resource status via Tilt API,
reads container logs, identifies common failure patterns.

## Steps

1. Check if Tilt is running:
   ```
   devdock_status()
   ```

2. If Tilt is running, identify unhealthy resources:
   - Look for resources with `runtime_status: "error"` or `update_status: "error"`
   - List all pending resources

3. For each failing resource, get logs:
   ```
   devdock_logs(service_name: "<name>", lines: 100)
   ```

4. Check for common failure patterns:
   - **Port conflicts**: `address already in use` → kill process on that port
   - **Missing images**: `image not found` → `docker pull` or check registry auth
   - **Bazel build errors**: `BUILD FAILED` → check build targets
   - **OOM**: `Killed` or `OOMKilled` → increase Docker memory
   - **Dependency timeout**: `connection refused` → check dependent services
   - **Config errors**: `missing key` or `invalid config` → check devdock.toml

5. Suggest fixes based on the identified pattern

## Quick checks

```bash
# Check Docker resources
docker stats --no-stream

# Check network connectivity
docker network inspect devdock_network

# Check if Tilt API responds
curl -s http://localhost:10350/api/view | python3 -c "import json,sys; [print(f'{r[\"metadata\"][\"name\"]:30s} {r[\"status\"][\"runtimeStatus\"]}') for r in json.load(sys.stdin).get('uiResources',[])]"
```
