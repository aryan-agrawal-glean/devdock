# Skill: check-tilt-status

Check the status of all Tilt resources for a running DevDock session.

## Usage
```
/check-tilt-status
```

## Steps

1. Query Tilt API and display resource status:
   ```bash
   curl -s http://localhost:10350/api/view > /tmp/tilt-status.json 2>/dev/null
   python3 -c "
   import json
   data = json.load(open('/tmp/tilt-status.json'))
   resources = data.get('uiResources', [])
   ok = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'ok')
   pending = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'pending')
   error = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'error')
   na = sum(1 for r in resources if r.get('status',{}).get('runtimeStatus') == 'not_applicable')
   total = len(resources)
   print(f'Total: {total} | OK: {ok} | Pending: {pending} | Error: {error} | N/A: {na}')
   print()
   for r in resources:
       s = r.get('status', {})
       name = r.get('metadata', {}).get('name', '?')
       rt = s.get('runtimeStatus', '?')
       up = s.get('updateStatus', '?')
       if rt in ('ok', 'error', 'pending'):
           print(f'  {name:30s} {rt:15s} {up}')
   print()
   # Show errors
   for r in resources:
       s = r.get('status', {})
       name = r.get('metadata', {}).get('name', '?')
       if s.get('updateStatus') == 'error':
           bh = s.get('buildHistory', [])
           if bh:
               err = bh[0].get('error', '')[:300]
               print(f'ERROR in {name}: {err}')
   " 2>/dev/null || echo "Tilt is not running"
   ```

2. Show Docker container status:
   ```bash
   docker ps --format "table {{.Names}}\t{{.Status}}" | grep devdock
   ```
