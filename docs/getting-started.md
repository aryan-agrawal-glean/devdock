# Getting Started with DevDock v2

## Installation

### macOS (Homebrew)

```bash
brew tap rahul-roy-glean/tap
brew install devdock
```

### From Source

Requires Rust 1.75+:

```bash
git clone https://github.com/rahul-roy-glean/devdock.git
cd devdock
cargo install --path .
```

## Quick Start

### 1. Test Your Installation

```bash
devdock --version
```

### 2. Validate a Profile

```bash
# If you have the Scio monorepo
cd ~/workspace/scio
devdock validate default_profile

# Or specify a profile path
devdock validate --profile ~/path/to/profile.yaml
```

### 3. Start DevDock

```bash
devdock start default_profile
```

This will:
- Parse and validate the profile
- Resolve dependencies
- Generate Tiltfile and configs
- Start Tilt with your services

### 4. Check Status

```bash
devdock status
```

### 5. Stop DevDock

```bash
devdock stop
```

## Next Steps

- [Profile Reference](profiles.md) - Learn about profile configuration
- [Commands](commands.md) - Complete command reference
- [Troubleshooting](troubleshooting.md) - Common issues and solutions
