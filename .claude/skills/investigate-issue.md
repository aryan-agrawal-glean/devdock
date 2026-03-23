# /investigate-issue

Investigate a bug or unexpected behavior in DevDock. Reads logs, traces
the lifecycle state machine, and identifies the root cause.

## Steps

1. Ask the user to describe the issue:
   - What command was run? (`devdock start <profile>`, `devdock stop`, etc.)
   - What was the expected behavior?
   - What actually happened? (error message, unexpected output, hang)

2. Check the lifecycle state machine for where it failed:
   - Read `crates/devdock-lifecycle/src/machine.rs` — which state transition panics or errors?
   - Read `crates/devdock-lifecycle/src/hooks.rs` — which hook is misbehaving?

3. Check the relevant command:
   - Commands live in `crates/devdock-cli/src/commands/`
   - Read the specific command file matching the failing command

4. Check if it's a config parsing issue:
   - `crates/devdock-core/src/project_config.rs` — devdock.toml parsing
   - `crates/devdock-core/src/service_config.rs` — devdock_config.json parsing
   - `crates/devdock-core/src/profile.rs` — profile YAML parsing

5. If Tilt is involved, check the generated Tiltfile:
   - Generated at `<repo>/devdock/runtime_generated/Tiltfile`
   - Generator: `crates/devdock-core/src/tiltfile_generator.rs`
   - Starlark libs: `lib/*.Tiltfile`

6. Reproduce the issue in a test fixture:
   - Use `tests/fixtures/` — pick the closest fixture
   - Add a new fixture if needed: `tests/fixtures/<scenario>/`

7. Write a failing test that captures the bug, then fix it.

## Key log locations

- DevDock background cquery: `~/.devdock/background-cquery.log`
- Tilt logs: visible in Tilt UI at http://localhost:10350
- Plugin stderr: captured and logged to devdock's stderr

## State machine states

```
Initializing → ProfileLoaded → PluginsReady → InfraReady
→ ConfigReady → TiltfileReady → TiltRunning
→ ResourcesStarting → Running → Stopping → Stopped
Any → Failed
```
