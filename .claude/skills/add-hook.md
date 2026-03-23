# /add-hook

Add a new lifecycle hook to DevDock. Updates the state machine, hook enum,
plugin protocol, and documentation.

## Steps

1. Ask the user:
   - Hook name (e.g., `after_metrics_collected`)
   - When it fires (which state transition)
   - What params it receives

2. Add the hook variant to `crates/devdock-lifecycle/src/hooks.rs`:
   ```rust
   pub enum Hook {
       // ... existing hooks
       AfterMetricsCollected,
   }
   ```

3. Add the fire method to `HookRunner` in `hooks.rs`:
   ```rust
   pub fn fire_after_metrics_collected(&mut self, params: &MetricsParams) -> Result<()> {
       self.fire(Hook::AfterMetricsCollected, params)
   }
   ```

4. Call the hook at the right point in `crates/devdock-lifecycle/src/machine.rs`.

5. Update the plugin protocol in `plugins/sdk/src/lib.rs`:
   - Add the new method name to the `handle()` match

6. Update `crates/devdock-core/src/project_config.rs`:
   - Add the hook name to `PluginHooks` struct if it's a global hook
   - Or to `ResourceHooks` if it's per-resource

7. Update `docs/architecture.md`:
   - Add the hook to the Hook Reference table
   - Add it to the lifecycle state machine diagram

8. Write a test in `crates/devdock-lifecycle/` that fires the hook and
   verifies the plugin receives it.

## Hook naming conventions

- `before_<event>` — fires before the event occurs
- `after_<event>` — fires after the event occurs
- `on_<condition>` — fires when a condition is met (e.g., `on_error`)
