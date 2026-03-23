# /test-profile

Test a DevDock profile end-to-end against a fixture repo or a real monorepo.
Validates that the profile starts cleanly, all resources reach a healthy state,
and stops cleanly.

## Usage

```
/test-profile <profile_name> [--repo <path>]
```

## Steps

1. Build and install the dev binary:
   ```bash
   cargo install --path crates/devdock-cli --debug --force
   devdock --version
   ```

2. Navigate to the target repo (default: `tests/fixtures/multi-service/`):
   ```bash
   cd <repo_path>
   ```

3. Clean any previous run artifacts:
   ```bash
   devdock stop 2>/dev/null || true
   rm -rf devdock/runtime_generated
   ```

4. Validate the profile first (fast check):
   ```bash
   devdock validate <profile_name>
   ```

5. Dry-run to preview what would start:
   ```bash
   devdock start <profile_name> --dry-run
   ```

6. Start the profile:
   ```bash
   devdock start <profile_name>
   ```

7. Wait 30-60 seconds for resources to start, then check Tilt resource status:
   ```bash
   curl -s http://localhost:10350/api/view > /tmp/tilt-status.json 2>/dev/null
   python3 -c "
   import json
   data = json.load(open('/tmp/tilt-status.json'))
   resources = data.get('uiResources', [])
   ok = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'ok')
   err = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'error')
   pending = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'pending')
   print(f'Total: {len(resources)} | OK: {ok} | Error: {err} | Pending: {pending}')
   for r in resources:
       s = r.get('status', {})
       name = r.get('metadata', {}).get('name', '?')
       rt = s.get('runtimeStatus', '?')
       up = s.get('updateStatus', '?')
       if rt in ('ok', 'error', 'pending'):
           print(f'  {name:35s}  runtime={rt:10s}  update={up}')
   for r in resources:
       s = r.get('status', {})
       if s.get('updateStatus') == 'error':
           bh = s.get('buildHistory', [])
           if bh:
               name = r.get('metadata', {}).get('name', '?')
               print(f'\\nERROR in {name}:')
               print(bh[0].get('error', '')[:500])
   " 2>/dev/null || echo "Tilt not yet running — wait and retry"
   ```

8. Stop when done:
   ```bash
   devdock stop
   ```

## Success Criteria

- `devdock validate` passes with no errors
- All infra resources: `runtimeStatus=ok`
- All local service builds: `updateStatus=ok`
- All local service containers: `runtimeStatus=ok`
- Remote services: `runtimeStatus=not_applicable` (expected)
- Zero resources with `error` status (ignoring transient build warnings)

## Test Fixture Repos

```bash
# Minimal single-service fixture
/test-profile default --repo tests/fixtures/minimal-python

# Multi-service fixture
/test-profile default --repo tests/fixtures/multi-service

# Infra-heavy fixture
/test-profile local --repo tests/fixtures/with-infra
```
