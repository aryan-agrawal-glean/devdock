"""Test suite configuration and setup utilities."""

load("bazel.Tiltfile", "bazel_build")
load("docker.Tiltfile", "generate_compose", "setup_docker_build")
load('logger.Tiltfile', 'export_logs')

def expand_env_vars(value):
    """Expand environment variables in a string value"""
    # Check if value is a string using type(str)
    if type(value) == "string":  # In Starlark, type() returns a string
        if '${' in value and value.endswith('}'):
            var_name = value[2:-1]
            return os.getenv(var_name, value)
        elif value.startswith('$') and not value.startswith('${'):
            var_name = value[1:]
            return os.getenv(var_name, value)
    return value

def load_test_suite_config(test_name):
    """Load test suite configuration from test_suite_config.yaml"""
    config_path = '{}/devdock/config/test_suite/test_suite_config.yaml'.format(os.getenv('DEVDOCK_ROOT'))
    if not os.path.exists(config_path):
        fail("Test suite config not found at: " + config_path)

    config = read_yaml(config_path)
    if test_name not in config:
        fail("Test suite config not found for test: " + test_name)

    return config[test_name]

def merge_test_suite_params(test_config, profile_params):
    """Merge test suite default params with profile-provided params"""
    merged_params = {}

    # Start with default params from test config
    if "params" in test_config:
        merged_params.update(test_config["params"])

    # Override with profile params
    if profile_params:
        merged_params.update(profile_params)

    return merged_params

def create_stability_check(test_name, dependency_name, stable_wait_seconds):
    """
    Create a local_resource that polls a dependency's Tilt status
    to ensure it's stable for the specified duration.

    Args:
        test_name: Name of the test suite
        dependency_name: Name of the dependency to check
        stable_wait_seconds: How long the service must be healthy

    Returns:
        stability_check_name: Name of the created stability check resource
    """
    stability_check_name = "{}-stability-check-{}".format(test_name, dependency_name)

    # Script that polls Tilt resource status
    check_script = """
set -e

DEP_NAME="{dep}"
STABLE_DURATION={duration}
CHECK_INTERVAL=1
MAX_RETRIES=300  # 5 minutes max wait
elapsed=0
consecutive_success=0
retries=0

echo "Waiting for $DEP_NAME to stabilize for ${{STABLE_DURATION}}s..."

while [ $elapsed -lt $STABLE_DURATION ]; do
  # Check if we've been waiting too long
  if [ $retries -ge $MAX_RETRIES ]; then
    echo "ERROR: $DEP_NAME did not stabilize after $((MAX_RETRIES * CHECK_INTERVAL)) seconds"
    exit 1
  fi

  # Get resource status
  if STATUS=$(tilt get uiresource "$DEP_NAME" -o json 2>/dev/null | jq -r '.status.runtimeStatus // "unknown"'); then
    case "$STATUS" in
      "ok"|"OK")
        consecutive_success=$((consecutive_success + CHECK_INTERVAL))
        elapsed=$((elapsed + CHECK_INTERVAL))
        echo "[$(date +%H:%M:%S)] ✓ $DEP_NAME healthy for ${{consecutive_success}}s / ${{STABLE_DURATION}}s"
        ;;
      "error"|"ERROR")
        echo "[$(date +%H:%M:%S)] ✗ $DEP_NAME in error state, resetting stability timer"
        consecutive_success=0
        elapsed=0
        ;;
      "pending"|"PENDING")
        echo "[$(date +%H:%M:%S)] ⏳ $DEP_NAME still pending, resetting stability timer"
        consecutive_success=0
        elapsed=0
        ;;
      *)
        echo "[$(date +%H:%M:%S)] ⚠ $DEP_NAME status unknown: $STATUS"
        consecutive_success=0
        elapsed=0
        ;;
    esac
  else
    echo "[$(date +%H:%M:%S)] ⚠ Failed to query $DEP_NAME status"
  fi

  retries=$((retries + 1))
  sleep $CHECK_INTERVAL
done

echo "✅ $DEP_NAME has been stable for {duration} seconds - ready to proceed!"
""".format(dep=dependency_name, duration=stable_wait_seconds)

    # Create a local resource that runs the stability check
    local_resource(
        stability_check_name,
        cmd=check_script,
        resource_deps=[dependency_name],
        labels=["stability-check"],
        auto_init=True
    )

    return stability_check_name

def setup_test(name, profile, project):
    """Set up a test suite execution environment"""

    test_name = name
    if profile.get('platform', '') == 'aws':
        test_name += '_aws'

    # Get architecture for image tag
    architecture = profile.get('architecture', 'unknown')

    # Load the test suite configuration
    test_suite_config = load_test_suite_config(test_name)
    test_suite_profile = profile.get("test_suite", {})

    # Get profile params if they exist
    profile_params = test_suite_profile.get(name, {}).get("params", {})

    # Merge params
    params = merge_test_suite_params(test_suite_config, profile_params)

    # Extract and merge extra mounts from profile
    global_mounts = test_suite_profile.get("extra_mounts", {})
    test_specific_mounts = test_suite_profile.get(name, {}).get("extra_mounts", {})

    # Get profile env vars if they exist
    global_env = test_suite_profile.get("env", {})
    test_specific_env = test_suite_profile.get(name, {}).get("env", {})

    # Merge environment variables (test-specific overrides global)
    profile_env = {}
    profile_env.update(global_env)
    profile_env.update(test_specific_env)

    # Merge mounts (test-specific overrides global)
    merged_mounts = {}
    merged_mounts.update(global_mounts)
    merged_mounts.update(test_specific_mounts)

    # Process mounts into docker-compose format
    extra_mounts_list = []
    for src, dst in merged_mounts.items():
        # Expand environment variables like ${DEVDOCK_ROOT}, ${HOME}
        expanded_src = expand_env_vars(src)
        mount_entry = "      - {}:{}".format(expanded_src, dst)
        extra_mounts_list.append(mount_entry)

    extra_mounts_str = "\n".join(extra_mounts_list) if extra_mounts_list else ""

    # Convert params to command line arguments
    # Note: param keys should include their own dashes (single or double) as configured in yaml
    params_list = []
    if params:
        for key, val in params.items():
            if val:
                params_list.append("{}={}".format(key, val))
            else:
                params_list.append("{}".format(key))
    params_str = " ".join(params_list)

    bazel_target = test_suite_config.get('bazel_target')

    # Run the Bazel build and get the binary path
    binary = bazel_build(
        name,
        bazel_target,
        [],
        type="test_suite",
        target_is_directory="True"
    )
    test_name = bazel_target.split(':')[-1]
    devdock_root = os.getenv('DEVDOCK_ROOT')

    file_path = bazel_target.split(':')[0].split('//')[-1] + "/" + test_name + ".py"
    local('cp {}/{} ~/.devdock/{}/{}.py'.format(devdock_root, file_path, name, test_name))
    local('cp {}/python_scio/scripts/tests/conftest.py ~/.devdock/{}/conftest.py'.format(devdock_root, name))
    mount_binary_path = "~/.devdock/{}:/app/{}".format(name, name)

    # Create necessary directories
    test_suite_dir = "{}/devdock/config/test_suite/{}".format(devdock_root, name)
    local("mkdir -p {}".format(test_suite_dir))

    # Expand environment variables in profile env
    expanded_profile_env = {}
    for k, v in profile_env.items():
        expanded_profile_env[k] = expand_env_vars(v)
    profile_env = expanded_profile_env

    # Get git user email for TRIGGERED_BY
    git_user_email = str(local('git config user.email', quiet=True)).strip()

    # Check if debug mode is enabled in the profile
    debug_mode = profile_env.get("DEBUG_MODE", "False")
    debug_port = None

    if debug_mode == "True":
        # Assign a debug port in the 6000-6099 range based on test name hash
        debug_port = 6000 + hash(name) % 100
        print("Debug mode enabled for test suite '{}' on port {}".format(name, debug_port))

    # Prepare default environment variables for docker-compose
    default_env_vars = {
        "GOOGLE_CLOUD_PROJECT": project,
        "PROJECT_NUMBER": profile.get('project_number', ''),
        "PROJECT_REGION": profile.get('region', ''),
        "PARAMS_STR": params_str,
        "RUNFILES_DIR": "/app/{test_name}/{test_name}.runfiles".format(test_name=test_name),
        "PATH": "/app/.venv/bin:/usr/bin:/usr/local/bin:/bin",
        "PYTHONPATH": "/app/{test_name}/{test_name}.runfiles/_main:/app/{test_name}/{test_name}.runfiles:/app/.venv/bin".format(test_name=test_name),
        "TEST_NAME": test_name,
        "GOOGLE_APPLICATION_CREDENTIALS": "/tmp/.config/gcloud/application_default_credentials.json",
        "IS_LOCAL_RUN": "true",
        "IS_DEVDOCK": "true",
        "TRIGGERED_BY": git_user_email,
        # Suppress Python warnings by default for all test suites
        "PYTHONWARNINGS": "ignore::UserWarning,ignore::FutureWarning",
        "PYTEST_DISABLE_WARNINGS": "1",
        # Default devdock service connection settings
        "MYSQL_HOST": "host.docker.internal",
        "MYSQL_PORT": "3310",
        "DSE_HOST": "host.docker.internal",
        "DSE_PORT": "7962",
    }

    # Add debug port to environment if debug mode is enabled
    if debug_port:
        default_env_vars["DEBUG_PORT"] = str(debug_port)

    if profile.get('platform', '') == 'aws':
        default_env_vars['AWS_REGION'] = profile.get('region')
        default_env_vars['AWS_ACCOUNT_ID'] = profile.get('aws_account_id')
        default_env_vars['AWS_ACCESS_KEY_ID'] = '${AWS_ACCESS_KEY_ID}'
        default_env_vars['AWS_SECRET_ACCESS_KEY'] = '${AWS_SECRET_ACCESS_KEY}'
        default_env_vars['AWS_SESSION_TOKEN'] = '${AWS_SESSION_TOKEN}'
        default_env_vars['RDS_SSL_CERT_PATH'] = '/tmp/global-bundle.pem'
        default_env_vars['CLOUD_PLATFORM'] = 'AWS'
        default_env_vars['PROJECT_NAME'] = profile.get('aws_account_id')
        default_env_vars['GOOGLE_CLOUD_PROJECT'] = profile.get('project')

    # Merge environment variables - profile env vars override defaults
    env_vars = {}
    env_vars.update(default_env_vars)
    env_vars.update(profile_env)

    # Format env vars for Dockerfile
    env_vars_str = "\n".join(['ENV {}="{}"'.format(k, v) for k, v in env_vars.items()])

    # Format env vars for docker-compose
    compose_env = "\n".join(['      {}: "{}"'.format(k, v) for k, v in env_vars.items()])

    # Determine base image tag based on architecture
    base_image = "us-central1-docker.pkg.dev/scio-engineering/glean-images/devdock_python_base"
    if architecture == 'linux':
        base_image += ":latest-amd64"

    # Set up Docker build context
    context = {
        "service": name,
        "base_image": base_image,
        "binary": binary,
        "mount_binary_path": mount_binary_path,
        "entrypoint_copy": "COPY entrypoint.sh /app/entrypoint.sh",
        "entrypoint_chmod": "RUN chmod +x /app/entrypoint.sh",
        "entrypoint_cmd": 'ENTRYPOINT ["/app/entrypoint.sh"]',
        "env_vars": env_vars_str,
        "compose_env": compose_env,
        "extra_mounts": extra_mounts_str,
        "port": "9123",  # Default port
        "debugport_hosts": "",
        "healthcheck": "",
        "type": "test_suite",
        "dockerfile_template": "{}/devdock/core/templates/Dockerfile.test_suite.tpl".format(devdock_root),
        "compose_template": "{}/devdock/core/templates/compose.test_suite.yaml.tpl".format(devdock_root)
    }

    # Add debug port mapping if debug mode is enabled
    if debug_port:
        context["debugport_hosts"] = '    ports:\n      - "{}:{}"'.format(debug_port, debug_port)

    # Set up Docker build
    docker_config = setup_docker_build(context, compose_file = context['compose_template'], dockerfile = context['dockerfile_template'])

    # Set up docker-compose
    compose_file = "{}/docker-compose.yaml".format(test_suite_dir)
    docker_compose(compose_file)

    # Enable log streaming
    export_logs(name, name)

    # Get test configuration
    test_config = test_suite_profile.get(name, {})

    # Get dependencies
    dependencies = test_config.get('dependencies', [])

    # Get stability configuration
    # Default to 10 seconds to ensure dependencies reach stability before tests start
    stable_wait_seconds = test_config.get('stable_wait_seconds', 10)
    stability_services = test_config.get('stability_check_services', dependencies)

    # Create stability check resources if configured
    stability_check_deps = []
    if stable_wait_seconds > 0:
        print("Configuring stability checks for test '{}' with {}s wait time".format(name, stable_wait_seconds))
        for dep_name in stability_services:
            # Skip non-service dependencies like 'init-gcloud-auth'
            if dep_name.startswith('init-') or '-bazel-build' in dep_name:
                continue

            print("  - Creating stability check for dependency: {}".format(dep_name))
            stability_check_name = create_stability_check(
                name,
                dep_name,
                stable_wait_seconds
            )
            stability_check_deps.append(stability_check_name)

    # Set up resource with dependencies including the Bazel build
    final_deps = [
        'init-gcloud-auth',
        '{}-bazel-build'.format(name),
    ]

    # Add stability checks if configured, otherwise use direct dependencies
    if stability_check_deps:
        final_deps.extend(stability_check_deps)
        print("Test '{}' will wait for {} stability checks to complete".format(name, len(stability_check_deps)))
    else:
        final_deps.extend(dependencies)

    resource = dc_resource(
        name,
        labels=["test_suite"],
        resource_deps=final_deps,
        auto_init=True
    )

    # Forward debug port if debug mode is enabled
    if debug_port:
        port_forward(int(debug_port), int(debug_port))

    return resource
