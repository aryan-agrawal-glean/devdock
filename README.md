# DevDock v2 - Rust-native , Blazing fast

A fast, extensible, standalone binary for managing local and remote development environments.

## 🚀 Features

- **Fast Startup** - Sub-30 second cold start (vs 2+ minutes with legacy system)
- **Standalone Binary** - No Python, Bash, or monorepo dependencies
- **Plugin System** - Extensible architecture for custom workflows
- **Multi-Environment** - Local Docker, remote Coder workspaces, and more
- **Profile-Based** - Declarative YAML profiles define your environment
- **Tilt Integration** - Leverages Tilt for battle-tested orchestration

## 📦 Installation

### Quick Install (Recommended)

**One-line installer** - automatically detects your OS and architecture:

```bash
curl -fsSL https://raw.githubusercontent.com/askscio/devdock/main/install.sh | bash
```

### Manual Installation

Download the binary for your platform from [GitHub Releases](https://github.com/askscio/devdock/releases/latest):

**macOS (Apple Silicon):**
```bash
curl -L https://github.com/askscio/devdock/releases/download/v0.1.0/devdock-macos-arm64-v0.1.0 -o devdock
chmod +x devdock
sudo mv devdock /usr/local/bin/
```

**macOS (Intel):**
```bash
curl -L https://github.com/askscio/devdock/releases/download/v0.1.0/devdock-macos-x86_64-v0.1.0 -o devdock
chmod +x devdock
sudo mv devdock /usr/local/bin/
```

**Linux (x86_64):**
```bash
curl -L https://github.com/askscio/devdock/releases/download/v0.1.0/devdock-linux-x86_64-v0.1.0 -o devdock
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
devdock --version  # Should output: devdock 0.1.0
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
- [Profile Reference](docs/profiles.md)
- [Plugin Development](docs/plugins.md)
- [Architecture](docs/architecture.md)

## 🏗️ Architecture

DevDock is built as a modular Rust workspace:

- `devdock-core` - Core types and abstractions
- `devdock-cli` - Command-line interface
- `devdock-config` - Profile parsing and validation
- `devdock-state` - State management (SQLite)
- `devdock-codegen` - Tiltfile and config generation

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
cargo run -- start default_profile

# Install locally
cargo install --path .
```

## 🤝 Contributing

Contributions welcome! Please see [CONTRIBUTING.md](CONTRIBUTING.md).

## 📄 License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.
