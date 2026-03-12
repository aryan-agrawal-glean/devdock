# Skill: build-and-test

Build the devdock binary and run all tests.

## Usage
```
/build-and-test
```

## Steps

1. Build all crates:
   ```bash
   cargo build --workspace
   ```

2. Run clippy (must be 0 warnings):
   ```bash
   cargo clippy --workspace -- -D warnings
   ```

3. Run all tests:
   ```bash
   cargo test --workspace
   ```

4. Verify no Glean-specific references:
   ```bash
   grep -rn "askscio\|dev-sandbox-334901\|scio-engineering\|glean-cluster\|salessavvy" crates/*/src/*.rs | grep -v "GLEAN_INSTANCE_NAME\|glean-apps-canary" | wc -l
   ```
   Should return 0.

5. Install dev binary:
   ```bash
   cargo install --path crates/devdock-cli --debug --force
   ```

## Success Criteria
- `cargo build` succeeds
- `cargo clippy` has 0 warnings
- All tests pass
- Zero Glean-specific references in crates/
- Binary installs successfully
