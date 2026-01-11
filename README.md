# DevDock v2 - Rust-native, Blazing fast

[![CI](https://github.com/askscio/devdock/actions/workflows/ci.yml/badge.svg)](https://github.com/askscio/devdock/actions/workflows/ci.yml)
[![License](https://img.shields.io/badge/license-MIT%2FApache--2.0-blue.svg)](LICENSE-MIT)
[![MSRV](https://img.shields.io/badge/MSRV-1.74-orange.svg)](https://www.rust-lang.org)

A fast, extensible, standalone binary for managing local and remote development environments.

## 🚀 Features

- **Fast Startup** - Sub-30 second cold start (vs 2+ minutes with legacy system)
- **Standalone Binary** - No Python, Bash, or monorepo dependencies
- **Plugin System** - Extensible architecture for custom workflows
- **Multi-Environment** - Local Docker, remote Coder workspaces, and more
- **Profile-Based** - Declarative YAML profiles define your environment
- **Tilt Integration** - Leverages Tilt for battle-tested orchestration

## 📦 Installation

### Prerequisites

- **GitHub CLI (`gh`)**: Required for private repository access
  ```bash
  # Install gh CLI if not already installed
  # macOS: brew install gh
  # Linux: https://cli.github.com/

  # Authenticate (one-time)
  gh auth login
  ```

### Quick Install (Recommended)

**One-line installer** - automatically detects your OS and architecture:

```bash
# Clone the repo and run installer (works with private repos)
git clone https://github.com/askscio/devdock.git && cd devdock && ./install.sh
```

**Or** download and run the installer script:

```bash
gh repo clone askscio/devdock
cd devdock
./install.sh
```

### Manual Installation

Download the binary for your platform from [GitHub Releases](https://github.com/askscio/devdock/releases/latest).

Extract and install:

```bash
# Download the appropriate release for your platform, then:
tar xzf devdock-*.tar.gz
chmod +x devdock
sudo mv devdock /usr/local/bin/
```

### From Source

```bash
git clone https://github.com/askscio/devdock.git
cd devdock
cargo install --path crates/devdock-cli
```

### Verify Installation

```bash
devdock --version
```

### Uninstall

```bash
curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/uninstall.sh | bash
```

## 🎯 Quick Start

```bash
# Start DevDock with a profile
devdock start default_profile

# Check status
devdock status

# Stop DevDock
devdock stop

# List all instances
devdock list
```

## 📚 Documentation

- [Getting Started](docs/getting-started.md)
- [Publishing Releases](docs/publishing.md)
- [Command Guide](COMMAND_GUIDE.md)

## 🏗️ Architecture

DevDock is built as a modular Rust workspace:

- `devdock-core` - Core types and abstractions
- `devdock-cli` - Command-line interface
- `devdock-config` - Profile parsing and validation
- `devdock-state` - State management (SQLite)
- `devdock-codegen` - Tiltfile and config generation
- `devdock-remote` - Remote workspace management (Coder integration)
- `devdock-sync` - File synchronization and watching
- `devdock-diagnostics` - Diagnostic reports and troubleshooting
- `devdock-monitor` - Health monitoring and notifications
- `devdock-telemetry` - Usage analytics and telemetry

## 🧑‍💻 Development

```bash
# Install Rust (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh

# Clone the repo
git clone https://github.com/askscio/devdock.git
cd devdock

# Build
cargo build

# Run tests
cargo test

# Run locally
cargo run -p devdock-cli -- start default_profile

# Install locally
cargo install --path crates/devdock-cli
```

## 🤝 Contributing

Contributions welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md).

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
