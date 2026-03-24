"""Generic build support for non-Bazel services.

Services that don't use Bazel can specify a `build_command` in their
devdock_config.json to run any build system (npm, cargo, make, etc.).

Example devdock_config.json:
  {
    "type": "node",
    "build_command": ["npm", "run", "build"],
    "build_output": "dist/",
    "binary_mount": "/app/dist",
    "watch_dirs": ["src/", "package.json"]
  }
"""

def generic_build(name, build_command, build_output, watch_dirs):
    """Create a local_resource that runs the given build command.

    The resource is named `{name}-build` and labelled `builds`.
    DevDock watches `watch_dirs` for changes and re-runs the command
    automatically, mirroring how Bazel watch triggers work.

    Args:
        name:          Service name (resource will be named `{name}-build`)
        build_command: List of command args, e.g. ["npm", "run", "build"]
        build_output:  Path to built artifact/directory (informational)
        watch_dirs:    Source directories/files to watch for changes
    """
    if not build_command:
        return

    cmd = ' '.join([str(c) for c in build_command])

    local_resource(
        '{}-build'.format(name),
        cmd=cmd,
        deps=watch_dirs,
        labels=['builds'],
        allow_parallel=True,
        dir=os.getenv('DEVDOCK_ROOT', ''),
    )
