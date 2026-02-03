# Grafana Observability Stack

This directory contains the configuration files for the DevDock rich logging stack (Grafana/Loki/Alloy).

## Files

- **docker-compose.yaml** - Docker Compose configuration for the full observability stack
- **loki-config.yaml** - Loki configuration with distributed architecture (read/write/backend)
- **alloy-local-config.yaml** - Grafana Alloy config for collecting Docker container logs
- **prometheus.yaml** - Prometheus configuration for metrics collection

## Architecture

The stack includes:

- **Loki (Read/Write/Backend)** - Distributed log aggregation system
  - Read service: Port 3101 (handles queries)
  - Write service: Port 3102 (handles log ingestion)
  - Backend: Index Gateway, Compactor, Ruler
- **Gateway (Nginx)** - Port 3100 (routes requests between read/write services)
- **Grafana** - Port 3000 (UI for log visualization)
- **Alloy** - Port 12345 (collects logs from Docker containers)
- **Minio** - S3-compatible storage for Loki data
- **Prometheus** - Port 9090 (metrics collection)

## Usage

### Enable Rich Logging

```bash
# Standalone command
devdock logs enable

# Or enable during startup
devdock start <profile> --enable-logs
```

### Check Status

```bash
devdock logs status
```

### Disable

```bash
devdock logs disable
```

## Data Storage

Log data is stored in `~/.devdock/grafana/.data/minio` with S3-compatible storage provided by Minio.

## Notes

- These configs are synced from the production scio monorepo
- Logs are retained for 168 hours (7 days) by default
- Alloy automatically scrapes logs from all Docker containers on the `devdock_network`
- Grafana is pre-configured with Loki and Prometheus as datasources
