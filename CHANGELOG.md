# Changelog

All notable changes to DevDock will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0] - 2025-12-29

### Added - First Release 🎉

#### Core Features
- ✅ **Fast Startup**: Sub-30 second cold start (vs 2+ minutes with Python-based DevDock)
- ✅ **Standalone Binary**: No Python, Bash, or monorepo dependencies required
- ✅ **Hybrid Watch Strategy**: Instant static patterns with background bazel cquery refinement
- ✅ **Multi-Platform Support**: macOS (ARM64/x86_64) and Linux (x86_64)
- ✅ **Profile-Based Configuration**: Declarative YAML profiles for environment management
- ✅ **Tilt Integration**: Leverages Tilt for battle-tested orchestration

#### CLI Commands
- `devdock start <profile>` - Start DevDock with a profile
- `devdock stop [target]` - Stop DevDock instance
- `devdock status` - Show DevDock status
- `devdock list` - List all instances with state tracking
- `devdock cleanup` - Full cleanup with database state reconciliation
- `devdock validate <profile>` - Validate a profile configuration
- `devdock generate <profile>` - Generate configuration without starting
- `devdock exec <container>` - Execute command in a container
- `devdock docker [check|networks|pull|login|prune]` - Docker management
- `devdock infra sql setup` - SQL infrastructure setup (replaces sql_hydrator.sh)
- `devdock infra sql clean` - Clean SQL databases/tables
- `devdock infra opensearch copy` - Copy OpenSearch indexes
- `devdock infra opensearch health` - Check OpenSearch health
- `devdock logs [enable|disable|status]` - Rich logging (Grafana/Loki/Alloy)
- `devdock service [enable|disable|logs]` - Service management
- `devdock profile [list|show|create]` - Profile management
- `devdock plugin [list|install|remove]` - Plugin management

#### Technical Improvements
- 🚀 **Parallel Config Generation**: 3 parallel tasks (INI, profile, watch files)
- 🚀 **Parallel Bazel Queries**: Background cquery for multiple services
- 🚀 **Parallel SQL Exports**: Simultaneous database exports from cloud
- 🗄️ **SQLite State Management**: Instance tracking with port allocation
- 🔄 **Silent Background Processing**: Logs to `~/.devdock/background-cquery.log`
- 🎯 **Smart Watch Files**: Only generates for local services (not infra or remote)
- 🧹 **Database State Cleanup**: Reconciles stale instances in cleanup command

#### Architecture
- **devdock-core**: Core types and abstractions (Profile, Service, etc.)
- **devdock-cli**: Command-line interface with clap
- **devdock-config**: Profile parsing, validation, and config generation
- **devdock-state**: SQLite-based state management and port allocation
- **devdock-codegen**: Tiltfile and Docker Compose generation

#### Replaced Python Components
- ✅ SQL hydrator (`devdock/config/infra/sql/util/`)
- ✅ OpenSearch index copier (`devdock/config/infra/opensearch/scripts/`)
- ✅ Config generators (INI, profile)
- ✅ Watch file generation (with multiple strategies)
- ✅ Profile validation
- ✅ GCS storage operations
- ✅ Platform abstraction (GCP, AWS, Azure)

#### Watch File Strategies
1. **Static** (default): Pre-computed glob patterns, instant (<1ms)
2. **Cquery**: Bazel query for exact dependencies (5-30s)
3. **Hybrid**: Static immediately + background cquery refinement
4. **Skip**: Disable watch file generation

#### Platform Support
- **Google Cloud Platform**: Full support with Cloud SQL Proxy and bastion tunneling
- **AWS**: Partial support (RDS, SSM)
- **Azure**: Stub implementation

### Fixed
- 🐛 Stale instances showing as "running" after crash or force-kill
- 🐛 Background cquery logs cluttering Tilt UI
- 🐛 Watch files generated for infrastructure components
- 🐛 Watch files generated for remote services

### Performance
- ⚡ 10x faster startup compared to Python implementation
- ⚡ Sub-millisecond watch file generation (static mode)
- ⚡ Non-blocking background refinement
- ⚡ Parallel configuration generation

### Documentation
- 📚 Getting started guide
- 📚 Command reference
- 📚 Architecture overview
- 📚 Release and installation instructions

---

[0.1.0]: https://github.com/rahul-roy-glean/devdock/releases/tag/v0.1.0
[Unreleased]: https://github.com/rahul-roy-glean/devdock/compare/v0.1.0...HEAD

