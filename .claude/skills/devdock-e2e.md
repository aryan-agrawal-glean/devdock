# /devdock-e2e

End-to-end test a DevDock feature from scratch: build the binary, wire in any
plugins, start a DevDock session, wait for full Tilt startup, verify the feature
works, check all logs, and clean up. Use this whenever you add or change a feature.

## Usage

```
/devdock-e2e <feature_description> [--profile <name>] [--repo <path>] [--plugin <path>]
```

Example:
```
/devdock-e2e "non-Bazel build_command support" --profile default --repo tests/fixtures/npm-service
/devdock-e2e "lifecycle hook after_ready fires" --profile default --repo tests/fixtures/multi-service
/devdock-e2e "plugin receives after_resource_start" --plugin plugins/notifications/slack-notifier
```

## Phase 1: Build

Build and install the dev binary:

```bash
cargo build --workspace 2>&1 | tail -5
# Check for errors — if any, STOP and fix them before proceeding

cargo install --path crates/devdock-cli --debug --force
devdock --version
```

Run fast checks:
```bash
cargo clippy --workspace -- -D warnings 2>&1 | grep "^error" | head -10
# Must be zero errors

cargo test --workspace 2>&1 | tail -20
# All tests must pass
```

## Phase 2: Plugin Setup (if testing a plugin)

First, detect the plugin language by looking at the plugin directory:
```bash
ls <plugin_dir>/
# Rust: Cargo.toml present
# Python: plugin.py or devdock-plugin.toml with language = "python"
# Go: go.mod present
# Bash: plugin.sh present
```

### Rust plugin

```bash
cd <plugin_dir>
cargo build --release
cd -

mkdir -p ~/.devdock/plugins
cp <plugin_dir>/target/release/<plugin-name> ~/.devdock/plugins/
chmod +x ~/.devdock/plugins/<plugin-name>
```

### Python plugin

No build step needed — Python plugins are scripts:

```bash
# Check Python version requirement
head -1 <plugin_dir>/plugin.py   # should be #!/usr/bin/env python3

# Install dependencies if any
if [ -f <plugin_dir>/requirements.txt ]; then
  pip install -r <plugin_dir>/requirements.txt
fi

# Install: make it executable and symlink/copy to plugins dir
mkdir -p ~/.devdock/plugins/<plugin-name>
cp -r <plugin_dir>/* ~/.devdock/plugins/<plugin-name>/
chmod +x ~/.devdock/plugins/<plugin-name>/plugin.py
```

Register using the full path to the script in `devdock.toml`:
```toml
[plugins.hooks]
after_ready = ["~/.devdock/plugins/<plugin-name>/plugin.py"]
```

### Go plugin

```bash
cd <plugin_dir>
go build -o <plugin-name> .
cd -

mkdir -p ~/.devdock/plugins
cp <plugin_dir>/<plugin-name> ~/.devdock/plugins/
chmod +x ~/.devdock/plugins/<plugin-name>
```

### Bash plugin

No build step needed:

```bash
mkdir -p ~/.devdock/plugins
cp <plugin_dir>/plugin.sh ~/.devdock/plugins/<plugin-name>
chmod +x ~/.devdock/plugins/<plugin-name>
```

### Verify the plugin responds to the protocol (all languages)

Regardless of language, every plugin must speak the JSON-lines protocol:

```bash
PLUGIN_PATH=~/.devdock/plugins/<plugin-name>  # or full path to script

echo '{"method":"initialize","config":{"project":"test","platform":"none","instance_id":"test-123"}}
{"method":"after_ready","params":{"session_id":"test-123","resource_count":3,"startup_duration_secs":12.5}}
{"method":"shutdown"}' | $PLUGIN_PATH

# Expected output (one JSON line per method):
# {"status":"ok"}
# {"status":"ok"}
# {"status":"ok"}
```

If the plugin outputs `{"status":"error","message":"..."}`, read the error and fix it
before continuing — DevDock will skip plugins that fail `initialize`.

### Register the plugin in `devdock.toml`

```toml
[plugins.hooks]
# Binary plugins (Rust, Go, Bash): just the binary name if in ~/.devdock/plugins/
after_ready = ["<plugin-name>"]

# Or full path (required for Python scripts and Bash scripts not in plugins dir):
after_ready = ["/absolute/path/to/plugin.py"]

# Per-resource hooks:
[plugins.resource_hooks.sql]
after_start = ["<plugin-name>"]
```

## Phase 3: Prepare the Test Repo

Navigate to the test repo:
```bash
cd <repo_path>   # default: tests/fixtures/multi-service
```

Clean any previous run artifacts:
```bash
devdock stop 2>/dev/null || true
rm -rf devdock/runtime_generated
```

Validate the profile:
```bash
devdock validate <profile>
# Must pass — fix any validation errors before continuing
```

Dry-run to verify the generated Tiltfile looks correct:
```bash
devdock start <profile> --dry-run
# Review the output: services, infra, plugins listed
# Check that the feature under test appears in the output
```

## Phase 4: Start DevDock

Start with full output:
```bash
devdock start <profile> 2>&1 | tee /tmp/devdock-e2e-start.log
```

Watch the lifecycle state transitions — should see:
```
[Initializing] → [ProfileLoaded] → [PluginsReady] → [InfraReady]
→ [ConfigReady] → [TiltfileReady] → [TiltRunning] → [ResourcesStarting]
→ [Running]
```

If any state fails, read `/tmp/devdock-e2e-start.log` for the error.

## Phase 5: Wait for Full Tilt Startup

Poll Tilt until all resources are healthy (max 5 minutes):

```bash
for i in $(seq 1 30); do
  STATUS=$(curl -s http://localhost:10350/api/view 2>/dev/null)
  if [ -z "$STATUS" ]; then
    echo "Attempt $i: Tilt not ready yet..."
    sleep 10
    continue
  fi
  COUNTS=$(echo "$STATUS" | python3 -c "
import json, sys
data = json.load(sys.stdin)
resources = data.get('uiResources', [])
ok = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'ok')
err = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'error')
pending = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'pending')
print(f'ok={ok} err={err} pending={pending} total={len(resources)}')
" 2>/dev/null)
  echo "Attempt $i: $COUNTS"
  if echo "$COUNTS" | grep -q "err=0" && echo "$COUNTS" | grep -q "pending=0"; then
    echo "All resources healthy!"
    break
  fi
  sleep 10
done
```

## Phase 6: Verify Feature

Check the full resource status table:

```bash
curl -s http://localhost:10350/api/view | python3 -c "
import json, sys
data = json.load(sys.stdin)
resources = data.get('uiResources', [])
print(f'Total resources: {len(resources)}')
print()
for r in resources:
    s = r.get('status', {})
    name = r.get('metadata', {}).get('name', '?')
    rt = s.get('runtimeStatus', '?')
    up = s.get('updateStatus', '?')
    labels = r.get('metadata', {}).get('labels', [])
    print(f'  {name:35s}  runtime={rt:12s}  update={up:8s}  labels={labels}')

# Print errors
print()
for r in resources:
    s = r.get('status', {})
    if s.get('runtimeStatus') == 'error' or s.get('updateStatus') == 'error':
        name = r.get('metadata', {}).get('name', '?')
        bh = s.get('buildHistory', [])
        err = bh[0].get('error', 'no error details') if bh else 'no build history'
        print(f'ERROR [{name}]: {err[:600]}')
"
```

**Feature-specific verification:** Based on what the feature does, verify it:
- New CLI flag → check it appears in `devdock --help` and changes behavior
- New lifecycle hook → check plugin received the hook call (see Phase 7)
- New service type support → check the service resource appears and is healthy
- Config change → check generated files in `devdock/runtime_generated/`
- Plugin integration → check plugin output in devdock logs

Check the generated Tiltfile for correctness:
```bash
cat devdock/runtime_generated/Tiltfile
```

Check the generated configs:
```bash
ls -la devdock/runtime_generated/
cat devdock/runtime_generated/temp_profile.yaml
```

## Phase 7: Check All Logs

**DevDock startup log:**
```bash
cat /tmp/devdock-e2e-start.log
```

**Tilt UI logs** (per resource):
```bash
# Get logs for a specific resource
curl -s "http://localhost:10350/api/view" | python3 -c "
import json, sys
data = json.load(sys.stdin)
for r in data.get('uiResources', []):
    name = r.get('metadata', {}).get('name', '?')
    s = r.get('status', {})
    bh = s.get('buildHistory', [])
    if bh and bh[0].get('log'):
        print(f'=== {name} ===')
        print(bh[0]['log'][-2000:])  # Last 2000 chars
        print()
"
```

**Plugin logs** (if testing a plugin):
```bash
# Plugins write to devdock's stderr — check the startup log
grep -i "plugin\|hook\|after_ready\|on_error" /tmp/devdock-e2e-start.log
```

**Background cquery log** (if testing Bazel):
```bash
cat ~/.devdock/background-cquery.log | tail -50
```

**Docker container logs** (if service is unhealthy):
```bash
docker ps --format "{{.Names}}" | grep devdock | while read name; do
  echo "=== $name ==="
  docker logs "$name" --tail 30 2>&1
done
```

## Phase 8: Pass/Fail Decision

**PASS criteria:**
- [ ] `cargo build` succeeded
- [ ] `cargo test` all passed
- [ ] `devdock start` reached `[Running]` state
- [ ] Tilt UI accessible at http://localhost:10350
- [ ] Zero resources with `error` status
- [ ] Feature-specific behavior verified (see Phase 6)
- [ ] No unexpected errors in any logs

**FAIL — stop and fix if:**
- Any resource stuck in `error` state for > 2 minutes
- `devdock start` exits before reaching `[Running]`
- Feature-specific check fails
- Plugin did not receive expected hooks

## Phase 9: Cleanup

```bash
devdock stop
rm -rf devdock/runtime_generated
rm -f /tmp/devdock-e2e-start.log /tmp/tilt-status.json
echo "Cleanup complete"
```

## Common Failure Patterns

| Symptom | Likely Cause | Fix |
|---------|-------------|-----|
| Tilt not starting | Port 10350 in use | `lsof -i :10350` → kill process |
| Infra resource error | Docker not running | `docker info` |
| Service build error | Wrong bazel_target or build_command | Check devdock_config.json |
| Plugin not firing | Not registered in devdock.toml | Add to `[plugins.hooks]` |
| Plugin binary not found | Wrong path or not built | Rebuild plugin, check `~/.devdock/plugins/` |
| State machine stuck | Hook timeout | Check plugin is responding to stdin |
| OOMKilled | Docker memory too low | Increase Docker memory limit |
