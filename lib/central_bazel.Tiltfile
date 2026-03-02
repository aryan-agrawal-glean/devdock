"""Central Bazel build resource for parallel compilation."""

load("utils.Tiltfile", "get_bazel_flags")

def setup_central_bazel_build(profile):
    """
    Creates a central bazel build resource that builds all non-Go service targets together.
    This allows Bazel to fully parallelize the build graph instead of building targets sequentially.

    Args:
        profile: The loaded profile YAML containing services configuration
    """
    devdock_root = os.getenv('DEVDOCK_ROOT')
    services = profile.get('services', {})
    architecture = profile.get('architecture', 'unknown')
    platform = profile.get('platform', 'google')

    targets = []

    for service_name in services.keys():
        if service_name in ['pre_setup', 'post_setup']:
            continue

        # Skip remote services - they don't need to be built
        service_profile = services.get(service_name, {})
        if type(service_profile) == "dict" and service_profile.get('type', 'local') == 'remote':
            continue

        # Load the service's devdock_config.json
        config = _load_service_config(devdock_root, service_name, platform)
        if not config:
            continue

        service_type = config.get('type', 'java')

        # Skip Go services (QE) - they have special build flags
        if service_type == 'go':
            continue

        bazel_target = config.get('bazel_target')
        if bazel_target:
            targets.append(bazel_target)

    if not targets:
        return

    # Build the bazel command
    bazel_command = ["bazel"]
    bazel_command.extend(get_bazel_flags())
    bazel_command.append("build")

    # Add jobs flag for Java on non-Linux
    if architecture != 'linux':
        bazel_command.append("--jobs=16")

    # Add all targets
    bazel_command.extend(targets)
    bazel_command_str = ' '.join(bazel_command)

    # Create resource deps list
    resource_deps_list = []
    if profile.get('infra', {}).get('sql', {}).get('type', 'remote') == 'local':
        if os.getenv('DEVDOCK_RUST_BINARY', '').lower() != 'true':
            resource_deps_list.append('sql-util-build')

    local_resource(
        'central-bazel-build',
        cmd = '''
            #!/bin/bash
            set -e
            SECONDS=0
            {bazel_command}
            bazel_status=$?
            echo "central bazel build took $SECONDS seconds"
            echo "Built {target_count} targets"
            exit $bazel_status
        '''.format(bazel_command=bazel_command_str, target_count=len(targets)),
        resource_deps = resource_deps_list,
        allow_parallel = True,
        labels = ['bazel-builds']
    )

def _load_service_config(devdock_root, service_name, platform):
    """Load a service's devdock_config.json file."""
    service_path = '{}/devdock/config/services/{}'.format(devdock_root, service_name)

    # Try platform-specific config first
    conf_path = '{}/devdock_config_{}.json'.format(service_path, platform)
    if not os.path.exists(conf_path):
        conf_path = '{}/devdock_config.json'.format(service_path)

    if not os.path.exists(conf_path):
        return None

    return read_json(conf_path)
