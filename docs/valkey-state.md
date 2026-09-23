# Valkey state in reused Linux workspaces

Profiles that include QE use local Valkey. Its persisted state is bind-mounted at
`~/.devdock/data/valkey`.

On Linux, `devdock start` prepares that DevDock-owned directory before Tilt starts
infrastructure. It uses a short-lived Docker helper with the Valkey image's expected
owner (`999:1000`) to recursively correct the bind mount ownership. The helper only
changes ownership; it preserves existing RDB contents. The Valkey service itself still
runs as its normal non-root image user.

If this preparation fails, DevDock stops before starting Tilt and reports the Docker
failure. Verify that the workspace user can access Docker and that
`valkey/valkey:8-alpine` is available, then rerun `devdock start`.
