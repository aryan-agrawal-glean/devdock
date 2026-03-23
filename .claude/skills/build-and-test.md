# /build-and-test

Build the full DevDock workspace, run all checks, and install the dev binary.

## Steps

1. Run the full build:
   ```bash
   cargo build --workspace
   ```

2. Run clippy (must be zero warnings):
   ```bash
   cargo clippy --workspace -- -D warnings
   ```

3. Run all tests:
   ```bash
   cargo test --workspace
   ```

4. Check formatting:
   ```bash
   cargo fmt --check
   ```
   If formatting is wrong, run `cargo fmt` and stage the changes.

5. Install dev binary for local testing:
   ```bash
   cargo install --path crates/devdock-cli --debug --force
   ```

6. Verify installation:
   ```bash
   devdock --version
   which devdock
   ```

## Running specific crate tests

```bash
cargo test -p devdock-core
cargo test -p devdock-config
cargo test -p devdock-core --test integration_test
cargo test -p devdock-lifecycle
```

## Common failures

- **Clippy warning**: Fix the warning directly — never add `#[allow(...)]` unless intentional
- **Test failure**: Read the failure output carefully; check `tests/fixtures/` for fixture issues
- **Fmt failure**: Run `cargo fmt`, then re-run `cargo fmt --check`
- **Linker error**: Missing system dep — check Cargo.toml `[build-dependencies]`
