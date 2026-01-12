# DevDock Tests

This directory contains integration tests and test fixtures for DevDock.

## Benchmark Tests

### Startup Time Benchmark

Located in `crates/devdock-cli/tests/benchmark_startup.rs`.

This benchmark measures the time to execute all startup logic (profile loading, validation, dependency resolution, config generation, Docker checks) up to but not including spawning the Tilt process.

**Running locally:**

```bash
cargo test --release benchmark_startup -- --ignored --nocapture
```

**Test Fixture:**

The minimal test fixture is located at `tests/fixtures/benchmark/` and includes:
- A Bazel workspace with 2 minimal services (service-a, service-b)
- A benchmark profile (`bench.yaml`) with minimal configuration
- Minimal BUILD.bazel files for each service

**CI Integration:**

The benchmark runs automatically on all PRs via the `benchmark` job in `.github/workflows/ci.yml`. It:
1. Benchmarks the PR branch
2. Benchmarks the main branch
3. Compares the results
4. **Fails the build** if the PR is >15% slower than main

**Regression Threshold:**

The current threshold is **15%**. This can be adjusted in the CI workflow if needed.

**Expected Performance:**

The minimal benchmark profile should complete in <10 seconds on CI runners.

## Test Fixtures

### `fixtures/benchmark/`

A minimal monorepo-like structure for benchmarking DevDock startup time. Contains:
- 2 services with minimal Bazel BUILD files
- A benchmark profile with basic infra and service configuration
- Minimal Tiltfile (not executed in dry-run mode)
