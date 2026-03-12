# Skill: test-profile

Test a DevDock profile end-to-end against the scio repository.

## Usage
```
/test-profile <profile_name>
```

## Steps

1. Ensure devdock binary is built and installed from current branch:
   ```bash
   cargo install --path crates/devdock-cli --debug --force
   ```

2. Navigate to scio repo and start the profile:
   ```bash
   cd ~/workspace/scio
   devdock stop 2>/dev/null
   rm -rf devdock/runtime_generated
   devdock start <profile_name>
   ```

3. Wait for Tilt to load and resources to stabilize (up to 5 minutes):
   ```bash
   sleep 60
   ```

4. Check resource status via Tilt API:
   ```bash
   curl -s http://localhost:10350/api/view > /tmp/tilt-status.json
   python3 -c "
   import json
   data = json.load(open('/tmp/tilt-status.json'))
   resources = data.get('uiResources', [])
   ok = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'ok')
   pending = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'pending')
   error = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'error')
   total = len(resources)
   print(f'Total: {total} | OK: {ok} | Pending: {pending} | Error: {error}')
   for r in resources:
       s = r.get('status', {})
       name = r.get('metadata', {}).get('name', '?')
       rt = s.get('runtimeStatus', '?')
       up = s.get('updateStatus', '?')
       if rt in ('ok', 'error', 'pending'):
           print(f'  {name:30s} {rt:15s} {up}')
   "
   ```

5. If there are errors, check logs:
   ```bash
   # For a specific resource
   curl -s http://localhost:10350/api/view > /tmp/tilt-status.json
   python3 -c "
   import json
   data = json.load(open('/tmp/tilt-status.json'))
   for r in data.get('uiResources', []):
       s = r.get('status', {})
       name = r.get('metadata', {}).get('name', '?')
       if s.get('updateStatus') == 'error':
           bh = s.get('buildHistory', [])
           if bh:
               print(f'ERROR in {name}: {bh[0].get(\"error\", \"unknown\")[:500]}')
   "
   ```

6. Stop devdock:
   ```bash
   cd ~/workspace/scio && devdock stop
   ```

## Success Criteria
- All infra resources: `runtime=ok`
- All local service builds: `update=ok`
- All local service containers: `runtime=ok`
- Remote services: properly skipped or port-forwarded
- Zero `error` status resources (excluding transient build issues)
