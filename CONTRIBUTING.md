# Contributing to DevDock

We welcome contributions! Here's how to get started.

## Development Setup

```bash
# Prerequisites
# - Rust 1.74+ (rustup.rs)
# - Docker
# - Tilt (docs.tilt.dev)

git clone https://github.com/aryan-agrawal-glean/devdock.git
cd devdock
cargo build
cargo test
```

## Making Changes

1. Fork the repo and create a branch: `git checkout -b my-feature`
2. Make your changes
3. Run checks: `cargo fmt && cargo clippy && cargo test`
4. Commit with a descriptive message
5. Open a PR

## Code Style

- Run `cargo fmt` before committing
- No clippy warnings: `cargo clippy --workspace -- -D warnings`
- All tests must pass: `cargo test --workspace`

## Adding a New Backend

To add a telemetry/storage/diagnostics backend:

1. Implement the trait in `crates/devdock-cli/src/backends.rs`
2. Add registration in `register_gcp_backends()` (or create a new register function)
3. Add the backend name to `devdock.toml` config options
4. Add a feature flag if the backend has external dependencies
5. Update README with the new backend option

Example — adding a Datadog telemetry backend:

```rust
// In backends.rs
pub struct DatadogTelemetry { api_key: String }

#[async_trait::async_trait]
impl TelemetrySyncBackend for DatadogTelemetry {
    fn name(&self) -> &str { "datadog" }
    async fn emit(&self, event: &TelemetryEvent) -> BackendResult<()> {
        // Send to Datadog API
        Ok(())
    }
    // ...
}
```

## Adding a New External Plugin Template

Plugin templates live in `crates/devdock-cli/src/commands/plugin.rs`. Add a new `generate_*_plugin()` function and wire it into the `create()` command.

## Project Structure

| Crate | Purpose |
|-------|---------|
| `devdock-core` | Core types, plugin traits, Tiltfile generator |
| `devdock-cli` | CLI commands, backend wrappers |
| `devdock-config` | Config generation (devdock.ini, profiles) |
| `devdock-remote` | Remote development (SSH, Coder) |
| `devdock-sync` | File synchronization |
| `devdock-monitor` | Health monitoring |
| `devdock-state` | SQLite state management |
| `devdock-telemetry` | Telemetry collection |
| `devdock-diagnostics` | Bug reporting |

## Testing

```bash
cargo test                              # All tests
cargo test -p devdock-core              # Core tests only
cargo test -p devdock-core --test integration_test  # Integration tests
```

Test fixtures are in `tests/fixtures/` — add new ones for testing different repo configurations.

## License

By contributing, you agree that your contributions will be licensed under the MIT/Apache-2.0 dual license.
