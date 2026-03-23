# /devdock-start

Start a DevDock profile. Validates the profile, checks Docker, and starts Tilt.

## Steps

1. Ask the user which profile to start (if not specified)
2. Validate the profile exists by checking `.devdock/profiles/`
3. If the MCP tool `devdock_start` is available, use it:
   ```
   devdock_start(profile: "<name>", dry_run: false)
   ```
4. Otherwise, run via CLI:
   ```bash
   devdock start <profile> --monitor
   ```
5. After starting, check resource health:
   ```
   devdock_status()
   ```
6. Report which resources are healthy, pending, or failed

## Common profiles

- `default` — All services + all infra
- `minimal` — Core services only
- `qe` — Query engine focused

## Troubleshooting

If startup fails:
- Check Docker is running: `docker info`
- Check port conflicts: `lsof -i :10350`
- Check Tilt logs: `docker logs devdock-tilt`
- Run with dry-run first: `devdock start <profile> --dry-run`
