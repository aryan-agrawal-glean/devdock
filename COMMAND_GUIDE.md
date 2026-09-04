# DevDock Command Guide & Cheatsheet

A comprehensive reference for all **implemented** DevDock CLI commands.

> 💡 **Quick Tip**: For fast lookups, check the [Quick Reference](#quick-reference) section below. For detailed explanations, see the [Detailed Commands](#detailed-commands) section.

> ⚠️ **Note**: This guide only includes commands that are fully implemented. Some commands defined in the CLI (like `plugin`, `service`, and `generate`) are not yet implemented and have been excluded.

---

## Quick Reference

### 🚀 Quick Start
```bash
devdock start <profile>              # Start local environment
devdock start <profile> --remote     # Start remote (Coder) environment
devdock start <profile> -m remote    # Legacy equivalent remote form
devdock guide <intent>                # Intent-based workflow guidance
devdock status                        # Check status
devdock stop                          # Stop current instance
```

### 📋 Basic Commands
```bash
devdock start <profile>               # Start with profile
  -s <services>                       # Start specific services (comma-separated)
  --remote                            # Remote mode shortcut
  -m remote                           # Remote mode (legacy equivalent)
  --auto-forward                      # Auto-forward ports (remote)
  --ci-mode                           # Non-interactive mode
  --enable-logs                       # Enable rich logging before starting

devdock stop [target]                 # Stop instance/service
  -f, --force                         # Force stop without cleanup

devdock status                        # Show status
  -d, --detailed                      # Detailed status

devdock list                          # List all instances
  -r, --running                       # Only running instances

devdock validate <profile>            # Validate profile
```

### 🐋 Docker Commands
```bash
devdock docker check                  # Check Docker status
devdock docker networks               # Setup Docker networks
devdock docker pull                   # Pull images
  --python-only                       # Python images only
  --cache                             # Save to cache
devdock docker login                  # Login to GCR
devdock docker prune                  # Remove dangling images
```

### 📝 Profile Management
```bash
devdock profile list                  # List profiles
devdock profile show <name>           # Show profile details
devdock profile apply                 # Apply profile/service changes without teardown
```

### 🗄️ Infrastructure - SQL
```bash
devdock infra sql setup               # SQL setup
  --project <name>                    # Project name (required)
  --platform <google|aws|azure>       # Platform (default: google)
  --region <region>                   # Region (default: us-central1)
  --db-tables <spec>                  # DB:tables specification
  --mode <fast|latest>                # Fast=hourly, latest=recent (default: fast)
  --force-copy                        # Force copy
  --full-cleanup                      # Full cleanup first
  --copy-keyval <true|false>          # Copy keyval table (default: true)

devdock infra sql clean               # Clean SQL
  --project <name>                    # Project name (required)
  --db-tables <spec>                  # DBs/tables to clean (empty=all)
```

### 🔍 Infrastructure - OpenSearch
```bash
devdock infra opensearch copy         # Copy indexes
  --project <name>                    # Project name (required)
  --indexes <list>                    # Comma-separated indexes (required)
  --query <json>                      # Filter query (default: match_all)
  --max-docs <num>                    # Max docs (0=unlimited, default: 0)

devdock infra opensearch health       # Check OpenSearch health
```

### 📊 Logging (Grafana/Loki)
```bash
devdock logs enable                   # Enable rich logging
devdock logs disable                  # Disable and cleanup
devdock logs status                   # Show logging status
```

### 🌐 Port Management (Remote)
```bash
devdock ports list                    # List port mappings
  --instance <id>                     # For specific instance

devdock ports forward                 # Forward ports
  --all                               # All common ports
  --service <name>                    # Specific service (qe, fe, tilt)
  --custom <local:remote>             # Custom mapping

devdock ports stop                    # Stop forwarding
  --all                               # Stop all
  --service <name>                    # Stop specific service
```

### 🔄 File Sync (Remote)
```bash
devdock sync start                    # Start sync (git-tracked only)
  --full                              # Sync all files
devdock sync stop                     # Stop sync
devdock sync status                   # Show sync status
```

### 🛠️ Utility Commands
```bash
devdock exec <container> [cmd]        # Execute in container
devdock cleanup                       # Full cleanup
  --soft                              # Service containers only
  --logs                              # Also clean logs
devdock -c                            # Cleanup alias

devdock setup                         # Initial setup
  --remote                            # Setup for remote mode

devdock guide                         # Show intent-based workflow guidance
  remote-start                        # Start/reuse remote DevDock
  add-service                         # Update services without full teardown
  sql-hydration                       # Hydrate or repair SQL
  file-sync                           # Inspect or force remote sync
  docker-auth                         # Fix image pull/auth issues
  troubleshoot                        # Status/report workflow
  cleanup                             # Safe cleanup guidance

devdock report                        # Report bug to Jira
  --summary <text>                    # Issue summary
  --description <text>                # Issue description
  --command <text>                    # Command run
```

### 🌍 Global Options
```bash
-v, -vv, -vvv                         # Increase verbosity
--version                             # Show version
--help                                # Show help
```

### 📌 Common Services
- `qe` - Query Engine
- `fe` - Frontend
- `crawler` - Web Crawler
- `docbuilder` - Document Builder
- `sql` - MySQL/MariaDB
- `opensearch` - OpenSearch
- `redis` - Redis
- `tilt` - Tilt UI (port 10350)

### 🔥 Quick Workflows

#### Local Development
```bash
devdock setup
devdock start my-profile
devdock status
devdock exec qe              # Opens shell
devdock stop
```

#### Remote Development
```bash
devdock setup --remote
devdock start my-profile --remote --auto-forward
devdock sync status
devdock stop
```

#### Infrastructure Setup
```bash
devdock infra sql setup --project my-proj --mode fast
devdock infra opensearch copy --project my-proj --indexes docs
devdock start my-profile
```

#### Troubleshooting
```bash
devdock status -d
devdock sync errors -n 20
devdock docker check
devdock guide troubleshoot
devdock cleanup && devdock start my-profile
devdock report              # If issue persists
```

### 💡 Quick Tips
- Use `devdock guide <intent>` when you know the workflow but not the exact flags
- Use `--mode fast` for SQL (hourly snapshots, much faster)
- Use `-s service1,service2` to start only needed services
- Use `--soft` cleanup for faster iteration
- Workspace name defaults to git branch in remote mode
- Use `--vm <name>` to reconnect to existing workspace
- File sync is automatic in remote mode

---

## Detailed Commands

### Table of Contents
- [Basic Commands](#basic-commands-1)
- [Docker Management](#docker-management-1)
- [Profile Management](#profile-management-1)
- [Intent-Based Guide](#intent-based-guide)
- [Infrastructure Commands](#infrastructure-commands)
- [Logging Management](#logging-management-1)
- [Port Management](#port-management-1)
- [File Sync Management](#file-sync-management-1)
- [Utility Commands](#utility-commands-1)

---

## Basic Commands

### Start DevDock
Start a DevDock environment with a profile.

```bash
# Basic local start
devdock start <profile>

# Start specific services only
devdock start <profile> -s qe,crawler,docbuilder

# Start in CI mode (non-interactive)
devdock start <profile> --ci-mode

# Start in remote mode (Coder workspace)
devdock start <profile> --remote

# Legacy equivalent remote form
devdock start <profile> -m remote

# Remote start with custom workspace name
devdock start <profile> --remote --workspace-name my-workspace

# Remote start with specific region
devdock start <profile> --remote --region us-west1-a

# Remote start with existing VM/workspace
devdock start <profile> --remote --vm workspace-name

# Remote start with auto port forwarding
devdock start <profile> --remote --auto-forward

# Copy your local GitHub credentials to a newly created remote workspace
devdock start <profile> --remote --copy-git-creds

# Remote start with custom port base
devdock start <profile> --remote --port-base 8000

# Start with verbose progress output
devdock start <profile> --verbose

# Enable rich logging (Grafana/Loki) before starting
devdock start <profile> --enable-logs
```

**Options:**
- `profile`: Profile name (e.g., `default_profile`)
- `-s, --services <services>`: Comma-separated list of services to start
- `--enable-logs`: Enable rich logging stack (Grafana/Loki) before starting DevDock
- `--ci-mode`: Run in non-interactive CI mode
- `--remote`: Shortcut for `--mode remote`
- `-m, --mode <mode>`: Start mode - `local` (default) or `remote`
- `--vm <name>`: Use existing Coder workspace
- `--workspace-name <name>`: Custom workspace name (default: branch name)
- `--region <region>`: GCP region for Coder workspace (default: `us-east4-a`)
- `--port-base <port>`: Custom port base for remote mode
- `--auto-forward`: Auto-forward all service ports (opt-in for remote)
- `--copy-git-creds`: For a newly created remote workspace, replace the user-level bot
  credential helper with local `gh` authentication. This is opt-in; without it, the Coder
  template's automation credentials remain unchanged. Requires local `git` and authenticated
  `gh` (`gh auth login --hostname github.com`) and cannot be combined with `--vm`.
- `--verbose`: Show detailed progress (all substeps)

---

### Stop DevDock
Stop a running DevDock instance.

```bash
# Stop current/latest instance
devdock stop

# Stop specific instance or service
devdock stop <instance-id>
devdock stop <service-name>

# Force stop without cleanup
devdock stop -f
devdock stop --force
```

**Options:**
- `target`: Optional instance ID or service name
- `-f, --force`: Force stop without cleanup

---

### Status
Show the current DevDock status.

```bash
# Basic status
devdock status

# Detailed status with more information
devdock status -d
devdock status --detailed
```

**Options:**
- `-d, --detailed`: Show detailed status information

---

### List Instances
List all DevDock instances.

```bash
# List all instances
devdock list

# List only running instances
devdock list -r
devdock list --running
```

**Options:**
- `-r, --running`: Show only running instances

---

### Validate Profile
Validate a profile configuration.

```bash
devdock validate <profile>
```

---

## Docker Management

### Check Docker
Verify Docker is running.

```bash
devdock docker check
```

---

### Setup Networks
Setup required Docker networks.

```bash
devdock docker networks
```

---

### Pull Images
Pull and update Docker images.

```bash
# Pull all required images
devdock docker pull

# Pull only Python images
devdock docker pull --python-only

# Pull images and save to cache
devdock docker pull --cache
```

**Options:**
- `--python-only`: Only pull Python images
- `--cache`: Save images to cache after pulling

---

### Docker Login
Login to Google Container Registry.

```bash
devdock docker login
```

---

### Prune Images
Remove dangling Docker images.

```bash
devdock docker prune
```

---

## Profile Management

### List Profiles
List all available profiles.

```bash
devdock profile list
```

---

### Show Profile Details
Display detailed information about a profile.

```bash
devdock profile show <profile-name>
```

---

### Apply Profile Changes
Apply profile/service changes to the running DevDock without tearing the whole environment down.

```bash
# Apply the running profile again, preserving current active services
devdock profile apply

# Apply a specific profile
devdock profile apply --profile <profile-name>

# Explicitly set active services
devdock profile apply --services qe,fe,docbuilder

# Preview changes without writing files or triggering resources
devdock profile apply --dry-run

# Remote apply uses the target running instance/workspace
devdock profile apply --instance <instance-id>
```

**Options:**
- `--profile <name>`: Profile to apply; defaults to the running instance profile
- `--instance <id>`: Target instance; defaults to the current workspace instance
- `-s, --services <services>`: Comma-separated active services to use
- `--dry-run`: Preview target/delta without writing files or triggering resources
- `--no-sql-refresh`: Do not trigger sql-hydrator even when local SQL is configured
- `-y, --yes`: Skip confirmation prompts

---

## Intent-Based Guide

Use the guide when you know the intent but not the exact command/flags. This is also the stable entrypoint agents should prefer before guessing DevDock flags.

```bash
# Show all supported intents
devdock guide

# Start/reuse remote DevDock
devdock guide remote-start

# Add or change services in a running DevDock
devdock guide add-service

# SQL hydration guidance
devdock guide sql-hydration

# Remote file sync guidance
devdock guide file-sync

# Docker auth/image pull guidance
devdock guide docker-auth

# Troubleshooting/reporting guidance
devdock guide troubleshoot

# Safe cleanup guidance
devdock guide cleanup
```

---

## Infrastructure Commands

### SQL Infrastructure

#### SQL Setup
Full SQL infrastructure setup (replaces legacy `sql_hydrator.sh` + `util.py`).

```bash
# Basic SQL setup
devdock infra sql setup --project <project-name>

# SQL setup with platform and region
devdock infra sql setup \
  --project my-project \
  --platform google \
  --region us-central1

# SQL setup with specific databases/tables
devdock infra sql setup \
  --project my-project \
  --db-tables "db1:table1,table2:query;db2:table3"

# Fast mode (hourly snapshot)
devdock infra sql setup --project my-project --mode fast

# Latest mode (most recent data)
devdock infra sql setup --project my-project --mode latest

# Force copy even if tables match
devdock infra sql setup --project my-project --force-copy

# Full cleanup before setup
devdock infra sql setup --project my-project --full-cleanup

# Skip keyval table copy
devdock infra sql setup --project my-project --copy-keyval false
```

**Options:**
- `--project <name>`: Project name (required)
- `--platform <platform>`: Platform - `google`, `aws`, or `azure` (default: `google`)
- `--region <region>`: Region (default: `us-central1`)
- `--db-tables <spec>`: Database/tables list in format `db:tables:query;db2:tables2`
- `--mode <mode>`: Mode - `fast` (hourly snapshot) or `latest` (default: `fast`)
- `--force-copy`: Force copy even if tables match
- `--full-cleanup`: Full cleanup before setup
- `--copy-keyval <bool>`: Copy keyval table (default: `true`)

---

#### SQL Clean
Clean SQL databases and tables.

```bash
# Clean all non-system databases
devdock infra sql clean --project <project-name>

# Clean specific databases/tables
devdock infra sql clean \
  --project my-project \
  --db-tables "db1:table1;db2:table2"

# Clean with platform and region
devdock infra sql clean \
  --project my-project \
  --platform google \
  --region us-central1
```

**Options:**
- `--project <name>`: Project name (required)
- `--platform <platform>`: Platform - `google`, `aws`, or `azure` (default: `google`)
- `--region <region>`: Region (default: `us-central1`)
- `--db-tables <spec>`: Database/tables to clean (empty = all non-system DBs)

---

### OpenSearch Infrastructure

#### Copy Indexes
Copy indexes from remote OpenSearch to local.

```bash
# Copy specific indexes
devdock infra opensearch copy \
  --project my-project \
  --indexes index1,index2,index3

# Copy with custom query filter
devdock infra opensearch copy \
  --project my-project \
  --indexes my-index \
  --query '{"match": {"field": "value"}}'

# Limit documents per index
devdock infra opensearch copy \
  --project my-project \
  --indexes my-index \
  --max-docs 1000
```

**Options:**
- `--project <name>`: Project name (required)
- `--indexes <list>`: Comma-separated list of indexes to copy (required)
- `--query <json>`: Query to filter documents (default: `{"match_all": {}}`)
- `--max-docs <num>`: Maximum documents per index (0 = unlimited, default: `0`)

---

#### Check OpenSearch Health
Check local OpenSearch health status.

```bash
devdock infra opensearch health
```

---

## Logging Management

Rich logging stack (Grafana/Loki/Alloy).

### Enable Logging
Enable the rich logging stack.

```bash
devdock logs enable
```

---

### Disable Logging
Disable and cleanup the rich logging stack.

```bash
devdock logs disable
```

---

### Logging Status
Show the current logging stack status.

```bash
devdock logs status
```

---

## Port Management

For remote DevDock instances (Coder workspaces).

### List Port Mappings
List all port mappings for remote instances.

```bash
# List all port mappings
devdock ports list

# List ports for specific instance
devdock ports list --instance <instance-id>
```

**Options:**
- `--instance <id>`: Show ports for specific instance ID

---

### Forward Ports
Enable port forwarding for remote DevDock.

```bash
# Forward all service ports (from profile) with offset
devdock ports forward --all

# Forward all service ports without port offset (local port = remote port)
devdock ports forward --all --no-offset

# Forward a retryable/stopped session by its Coder workspace
devdock ports forward --workspace <workspace-name> --all --no-offset

# Forward specific service port
devdock ports forward --service qe
devdock ports forward --service fe
devdock ports forward --service tilt

# Forward custom port mapping
devdock ports forward --custom 8080:80
devdock ports forward --custom 3000:3000
```

**Options:**
- `--all`: Forward all service ports discovered from the instance's profile
- `--workspace <name>`: Resolve a remote session by immutable Coder workspace identity; works for retryable stopped sessions
- `--service <name>`: Forward specific service port (e.g., `qe`, `fe`, `tilt`)
- `--custom <mapping>`: Custom port mapping in format `local:remote`
- `--no-offset`: Don't apply port_base offset to local ports (use remote port as local port)

---

### Stop Port Forwarding
Stop port forwarding for remote DevDock.

```bash
# Stop all port forwards
devdock ports stop --all

# Stop specific service port forward
devdock ports stop --service qe
devdock ports stop --service tilt
```

**Options:**
- `--all`: Stop all port forwards
- `--service <name>`: Stop specific service port forward

---

### Recover a Manually Started Remote Session
Reconcile a stopped local session after DevDock was started manually inside Coder. Recovery is
fail-closed and requires matching tmux, Tilt, and `DEVDOCK_ID` evidence before marking the session
running. It also attaches all service forwards on their remote ports and verifies local listeners.

```bash
devdock recover --workspace <workspace-name>
```

For a handoff between two existing remote sessions, stop only verified forwards from the source and
probe that their loopback ports are free before attaching the target:

```bash
devdock recover --workspace <to-workspace> --from-workspace <from-workspace>
```

If there is no local session row yet, provide the profile once:

```bash
devdock recover --workspace <workspace-name> --profile <profile-name>
```

---

## File Sync Management

For remote DevDock instances (Coder workspaces).

### Start File Sync
Start file synchronization (auto-started with remote mode).

```bash
# Start sync (git-tracked files only)
devdock sync start

# Start full sync (all files)
devdock sync start --full
```

**Options:**
- `--full`: Sync all files instead of git-tracked only

---

### Stop File Sync
Stop file synchronization.

```bash
devdock sync stop
```

---

### Sync Status
Show file sync status and statistics.

```bash
devdock sync status
```

---

## Utility Commands

### Execute Command in Container
Execute a command inside a running container.

```bash
# Open shell in container
devdock exec <container-name>

# Execute specific command
devdock exec <container-name> "ls -la"
devdock exec qe "python --version"

# Special case: MySQL prompt in sql container
devdock exec sql
```

**Common containers:**
- `qe`: Query Engine
- `fe`: Frontend
- `crawler`: Crawler service
- `sql`: MySQL/MariaDB
- `opensearch`: OpenSearch
- `redis`: Redis

---

### Cleanup
Full cleanup of containers, networks, and files.

```bash
# Full cleanup (containers + infra)
devdock cleanup

# Soft cleanup (service containers only, not infra)
devdock cleanup --soft

# Cleanup including old logs
devdock cleanup --logs

# Alias for cleanup
devdock -c
```

**Options:**
- `--soft`: Only clean service containers (not infrastructure)
- `--logs`: Also clean old log files

---

### Setup
Initial setup of DevDock environment.

```bash
# Setup for local DevDock
devdock setup

# Setup for remote DevDock (Coder workspaces)
devdock setup --remote
```

**Options:**
- `--remote`: Setup for remote DevDock (Coder workspaces)

---

### Report Bug
Report a bug to Jira (interactive).

```bash
# Interactive bug report (prompts for details)
devdock report

# Non-interactive bug report with all details
devdock report \
  --summary "Brief issue description" \
  --description "Detailed description of the issue" \
  --command "devdock start my-profile"
```

**Options:**
- `--summary <text>`: Brief summary of the issue (skip interactive prompt)
- `--description <text>`: Detailed description (skip interactive prompt)
- `--command <text>`: Command that was run when issue occurred (skip interactive prompt)

---

## Global Options

These options work with any command:

```bash
# Increase logging verbosity
devdock -v <command>     # Info level
devdock -vv <command>    # Debug level
devdock -vvv <command>   # Trace level

# Show version
devdock --version

# Show help
devdock --help
devdock <command> --help
```

---

## Common Workflows

### Local Development Workflow
```bash
# 1. Initial setup
devdock setup

# 2. Start your environment
devdock start my-profile

# 3. Check status
devdock status

# 4. Execute commands in containers
devdock exec qe "python manage.py shell"

# 5. Stop when done
devdock stop
```

---

### Remote Development Workflow
```bash
# 1. Setup remote DevDock
devdock setup --remote

# 2. Start remote environment
devdock start my-profile -m remote --auto-forward

# 3. Check sync status
devdock sync status

# 4. Forward additional ports if needed
devdock ports forward --service tilt

# 5. Work on your code (auto-synced)

# 6. Stop when done
devdock stop
```

---

### Infrastructure Setup Workflow
```bash
# 1. Setup SQL infrastructure
devdock infra sql setup \
  --project my-project \
  --mode fast \
  --db-tables "mydb:users,posts"

# 2. Setup OpenSearch indexes
devdock infra opensearch copy \
  --project my-project \
  --indexes documents,users

# 3. Check OpenSearch health
devdock infra opensearch health

# 4. Start your profile
devdock start my-profile
```

---

### Troubleshooting Workflow
```bash
# 1. Check status
devdock status -d

# 2. Verify Docker is running
devdock docker check

# 3. View instance list
devdock list

# 4. Check container logs directly
devdock exec qe "tail -f /var/log/app.log"

# 5. If stuck, try cleanup and restart
devdock cleanup
devdock start my-profile

# 6. Report bug if issue persists
devdock report
```

---

## Tips & Tricks

### Service Name Shortcuts
Common service names you'll use:
- `qe` - Query Engine
- `fe` - Frontend
- `crawler` - Web Crawler
- `docbuilder` - Document Builder
- `sql` - MySQL/MariaDB database
- `opensearch` - OpenSearch cluster
- `redis` - Redis cache
- `tilt` - Tilt UI (usually at port 10350)

### Profile Names
Profiles are typically stored in `~/.devdock/profiles/` or in your workspace's `.devdock/profiles/` directory.

### Remote Mode Tips
- Workspace name defaults to your current git branch
- Use `--vm` to reconnect to an existing workspace
- Use `--auto-forward` to automatically forward common ports
- File sync happens automatically in the background

### Performance Tips
- Use `--mode fast` for SQL setup to use hourly snapshots (much faster)
- Use `-s` to start only the services you need
- Use `--soft` cleanup for faster iteration (keeps infra running)
- Use `devdock -c` as a quick cleanup alias

---

## Environment Variables

DevDock respects the following environment variables:

- `DEVDOCK_HOME` - DevDock home directory (default: `~/.devdock`)
- `DEVDOCK_PROFILE_DIR` - Custom profile directory
- `DOCKER_HOST` - Docker daemon socket
- `CODER_URL` - Coder server URL (for remote mode)
- `CODER_TOKEN` - Coder authentication token

---

## Exit Codes

- `0` - Success
- `1` - General error
- `2` - Invalid arguments or usage
- `130` - Interrupted by user (Ctrl+C)

---

## Getting Help

```bash
# General help
devdock --help

# Command-specific help
devdock start --help
devdock infra sql setup --help

# Report a bug
devdock report

# Check version
devdock --version
```

---

## Additional Resources

- [Getting Started Guide](docs/getting-started.md)
- [Profile Reference](docs/profiles.md)
- [Plugin Development](docs/plugins.md)
- [Architecture Documentation](docs/architecture.md)
- [GitHub Repository](https://github.com/askscio/devdock)

---

*Last updated: January 2026*
*DevDock version: 0.1.0+*
