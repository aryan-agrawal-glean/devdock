# Utility functions for DevDock Tiltfiles

def get_bazel_cmd():
    """
    Get bazel command with architecture-specific flags.

    Returns:
        str: "bazel" with appropriate startup flags based on architecture
    """
    devdock_root = os.getenv('DEVDOCK_ROOT')
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    architecture = profile.get('architecture', 'unknown')

    if architecture == 'linux':
        return 'bazel --nohome_rc --bazelrc={}/.devdock.remote.bazelrc'.format(devdock_root)
    else:
        return 'bazel'

def get_bazel_flags():
    """
    Get bazel startup flags based on architecture.

    Returns:
        list: List of bazel startup flags, empty list for non-Linux systems
    """
    devdock_root = os.getenv('DEVDOCK_ROOT')
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    architecture = profile.get('architecture', 'unknown')

    if architecture == 'linux':
        return ['--nohome_rc', '--bazelrc={}/.devdock.remote.bazelrc'.format(devdock_root)]
    else:
        return []
