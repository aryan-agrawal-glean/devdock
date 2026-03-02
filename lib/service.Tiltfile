"""Service configuration and setup utilities."""

load("bazel.Tiltfile", "bazel_build")
load("docker.Tiltfile", "generate_compose", "setup_docker_build")
load('logger.Tiltfile', 'export_logs')
load('../../config/services/service_hooks/Tiltfile', 'get_pre_service_hooks')

DEVDOCK_GCLOUD_VAULT_PATH = '~/.devdock/vault_key'
DUMMY_METRIC_PORT = 10000

ENV_TO_PROFILE_MAP = {
    'GOOGLE_CLOUD_PROJECT': 'project',
    'SQL_ACCOUNT': 'sql_account',
    'AWS_ACCOUNT_ID': 'aws_account_id',
    'REGION': 'region',
    'GLEAN_INSTANCE_NAME': 'glean_instance_name',
    'AZURE_VERSIONED_STORAGE_ACCOUNT': 'azure_versioned_storage_account',
    'AZURE_UNVERSIONED_STORAGE_ACCOUNT': 'azure_unversioned_storage_account',
    'ARM_SUBSCRIPTION_ID': 'subscription_id',
    'K8S_SERVICE_ACCOUNT': 'k8s_service_account'
}

def expand_env_vars(value, profile):
    """Expand environment variables in a string value"""
    # Check if value is a string using type(str)
    if type(value) == "string":  # In Starlark, type() returns a string
        if '${' in value:
            expanded = profile.get(ENV_TO_PROFILE_MAP.get(value[2:-1], None), None) or os.getenv(value[2:-1], value)
            return expanded
        elif value.startswith('$'):
            expanded = profile.get(ENV_TO_PROFILE_MAP.get(value[1:], None), None) or os.getenv(value[1:], value)
            return expanded
    return value

def load_service_config(path):
    """Load service configuration from a JSON file"""

    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(os.getenv('DEVDOCK_ROOT')))
    platform = profile.get('platform', 'google')
    conf_path = path + '/devdock_config_' + '{}.json'.format(platform)
    if not os.path.exists(conf_path):
        conf_path = path + '/devdock_config.json'
    config = read_json(conf_path)

    # Expand environment variables in env section
    if 'env' in config:
        expanded_env = {}
        for k, v in config['env'].items():
            expanded_env[k] = expand_env_vars(v, profile)
        expanded_env['SQL_ACCOUNT'] = expand_env_vars('${SQL_ACCOUNT}', profile)
        config['env'] = expanded_env

    return config

def setup_service(name, config, auto_init=True):
    """Set up a service with its dependencies"""

    # Set up service directory path
    service_dir = "services/" + name

    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(os.getenv('DEVDOCK_ROOT')))

    # Build the service using Bazel
    binary = bazel_build(name, config["bazel_target"], config.get("watch_dirs", []), config.get("type", ""), config.get("target_is_directory", "True"), profile.get('services', {}).get(name, {}).get('hot_reload', 'enable'), config.get("copy_runfiles", False))
    binary_mount = "{}:{}".format(binary, config.get("binary_mount", ""))

    # Get port configuration
    port = config.get("env", {}).get("PORT", "8080")

    service_env = profile.get('services').get(name, {}).get('env', {})

    # Prepare environment variables for docker-compose
    env_list = []
    for key, val in config.get("env", {}).items():
        if key in service_env:
            env_list.append("      {}: '{}'".format(key, service_env[key]))
            service_env.pop(key, None)
        else:
            env_list.append("      {}: '{}'".format(key, val))

    # Inject DevDock service name for log labeling (e.g., "crawler", "docbuilder")
    if 'DEVDOCK_SERVICE_NAME' not in config.get("env", {}) and 'DEVDOCK_SERVICE_NAME' not in service_env:
        env_list.append("      DEVDOCK_SERVICE_NAME: '{}'".format(name))

    # Add additional user specified env env_vars
    for key, val in service_env.items():
        env_list.append("      {}: '{}'".format(key, service_env[key]))

    env_vars_compose = "\n".join(env_list) if env_list else ""

    # Prepare environment variables for Dockerfile
    env_vars = []
    for k, v in config.get("env", {}).items():
        env_vars.append("ENV {}={}".format(k, v))

    if profile.get('enable_service_metrics', 'False') == 'True':
        env_vars.append("ENV DEVDOCK_METRICS_CAPTURE_ENABLED=True")

    env_vars = "\n".join(env_vars)

    # Prepare extra mounts and volumes
    extra_mounts = []
    volumes_config = []
    for src, dst in config.get("extra_mounts", {}).items():
        # Replace variables in source path
        src_path = src.replace("${SERVICE_DIR}", service_dir)
        mount = "      - {0}:{1}".format(src_path, dst)
        extra_mounts.append(mount)

    vault_key_path = profile.get('vault_key', None)
    if vault_key_path and vault_key_path != '':
        src_path = '{}/application_default_credentials.json'.format(DEVDOCK_GCLOUD_VAULT_PATH)
        local('mkdir -p {} && rsync -rltD -L {} {}/application_default_credentials.json'.format(DEVDOCK_GCLOUD_VAULT_PATH, vault_key_path, DEVDOCK_GCLOUD_VAULT_PATH))
        extra_mounts.append("      - {}/application_default_credentials.json:/tmp/.config/gcloud/application_default_credentials.json".format(DEVDOCK_GCLOUD_VAULT_PATH))

    extra_mounts = "\n".join(extra_mounts) if extra_mounts else ""
    volumes = "\n".join(volumes_config) if volumes_config else ""

    # Prepare debug port configuration
    debug_port = config.get("debug_port")
    debugport_expose = "EXPOSE " + str(debug_port) if debug_port else ""

    extra_hosts_entries = []

    if debug_port:
        extra_hosts_entries += ['host.docker.internal:host-gateway', 'local.glean.com:host-gateway']

    if profile.get('services', {}).get('composio_apollo', {}).get('type', '') == 'remote':
        extra_hosts_entries += [
            'composio-apollo.composio.svc.cluster.local:host-gateway',
            'composio-apollo:host-gateway',
            'composio-mcp.composio.svc.cluster.local:host-gateway',
        ]

    debugport_hosts = ""
    if extra_hosts_entries:
        debugport_hosts = "extra_hosts:\n      - " + "\n      - ".join(extra_hosts_entries)

    # Handle entrypoint configuration
    entrypoint_config = config.get("entrypoint", {})
    if entrypoint_config:
        script_path = entrypoint_config.get("script", "")
        if script_path:
            # Ensure script has correct line endings and is executable
            if os.path.exists(script_path):  # Only chmod if file exists
                local('chmod +x {}'.format(script_path))

            entrypoint_copy = "COPY {} /app/entrypoint.sh".format(script_path)
            entrypoint_chmod = "RUN chmod +x /app/entrypoint.sh"
            entrypoint_args = entrypoint_config.get("args", [])
            args_str = ""
            for arg in entrypoint_args:
                args_str += ', "{}"'.format(arg)
            if args_str:
                entrypoint_cmd = 'ENTRYPOINT ["/app/entrypoint.sh"{}]'.format(args_str)
            else:
                entrypoint_cmd = 'ENTRYPOINT /app/entrypoint.sh'
        else:
            entrypoint_copy = ""
            entrypoint_chmod = ""
            entrypoint_cmd = ""
    else:
        # Default Maven WAR entrypoint
        entrypoint_copy = ""
        entrypoint_chmod = ""
        if not entrypoint_config:
            debug_env = 'ENV MAVEN_OPTS="-agentlib:jdwp=transport=dt_socket,server=y,suspend=n,address=0.0.0.0:{}"\n'.format(debug_port) if debug_port else ""
            entrypoint_cmd = debug_env + 'ENTRYPOINT mvn jetty:deploy-war -Djetty.http.port={} -Djetty.scanner.scanIntervalSeconds=3 --no-transfer-progress'.format(
                config['env']['PORT']
            )

    # Initialize healthcheck with empty string
    healthcheck = ""

    # Handle healthcheck configuration
    healthcheck_config = config.get("healthcheck")
    if healthcheck_config:
        if type(healthcheck_config) == "str" and healthcheck_config:
            # Simple URL healthcheck
            healthcheck = """    healthcheck:
      test: [ "CMD", "curl", "-f", "{}" ]
      start_period: 0s
      start_interval: 0s
      interval: 1s
      timeout: 4s
      retries: 1""".format(healthcheck_config)
        elif type(healthcheck_config) == "dict" and "test" in healthcheck_config:
            # Just handle the test command, keep other parameters constant
            test_cmd = healthcheck_config["test"]
            if type(test_cmd) == "str" and test_cmd:
                # If it's a string, wrap it in CMD-SHELL
                healthcheck = """    healthcheck:
      test: [ "CMD-SHELL", "{}" ]
      start_period: 0s
      start_interval: 0s
      interval: 1s
      timeout: 4s
      retries: 1""".format(test_cmd)
            elif type(test_cmd) == "list" and len(test_cmd) > 0:
                # Assume it's a list starting with CMD
                healthcheck = """    healthcheck:
      test: {}
      start_period: 0s
      start_interval: 0s
      interval: 1s
      timeout: 4s
      retries: 1""".format(test_cmd)

    platform = profile.get('platform', 'google')

    # Determine Go architecture based on machine architecture
    arch = profile.get('architecture', 'unknown')
    if 'linux' in arch:
        go_arch = 'amd64'
        aws_cli_arch = 'x86_64'
    elif 'darwin' in arch:
        go_arch = 'arm64'
        aws_cli_arch = 'aarch64'
    else:
        go_arch = 'arm64'
        aws_cli_arch = 'aarch64'

    # Combine platform-specific defaults with config run_commands
    platform_defaults = []
    if platform == 'azure':
        platform_defaults.append('RUN curl -sL https://aka.ms/InstallAzureCLIDeb | bash')
    elif platform == 'aws':
        platform_defaults.append('RUN apt-get update && apt-get install -y unzip && curl "https://awscli.amazonaws.com/awscli-exe-linux-{}.zip" -o "awscliv2.zip" && unzip -o awscliv2.zip && ./aws/install'.format(aws_cli_arch))

    config_run_commands = config.get("run_commands", [])
    all_run_commands = platform_defaults + config_run_commands
    run_commands = '\n'.join(all_run_commands) if all_run_commands else ''

    # Determine service type early for base image handling
    service_type = config.get("type", "java")  # Default to java for backward compatibility

    # Set up Docker build context
    base_image = config.get("base_image", "eclipse-temurin:17-jdk")

    # For Python services on Linux, append architecture tag if not already present
    if service_type == "python" and arch == 'linux':
        if not base_image.endswith(":latest-amd64") and not base_image.endswith("-amd64"):
            base_image += ":latest-amd64"

    context = {
        "service": name,
        "binary": binary,
        "mount_binary_path": binary_mount,
        "base_image": base_image,
        "port": port,
        "debug_port": debug_port,
        "env_vars": env_vars,
        "compose_env": env_vars_compose,
        "extra_mounts": extra_mounts,
        "debugport_expose": debugport_expose,
        "debugport_hosts": debugport_hosts,
        "healthcheck": healthcheck,
        "entrypoint_copy": entrypoint_copy,
        "entrypoint_chmod": entrypoint_chmod,
        "entrypoint_cmd": entrypoint_cmd,
        "run_commands": run_commands,
        "type": config.get("type", "java"),
        "metrics_port": config.get("metrics_port", DUMMY_METRIC_PORT),
        "go_arch": go_arch,
    }

    devdock_root = os.getenv('DEVDOCK_ROOT')

    # For Python services, copy the appropriate requirements file based on architecture
    if service_type == "python":
        architecture = profile.get('architecture', 'unknown')
        service_path = "{}/devdock/config/services/{}".format(devdock_root, name)
        if architecture == 'linux':
            # Use Linux-specific requirements
            local('cp {}/python_scio/requirements/bazel/linux/requirements.txt {}/requirements.txt'.format(devdock_root, service_path))
        else:
            # Use default requirements (for darwin/arm64)
            local('cp {}/python_scio/requirements/bazel/linux_aarch64/requirements.txt {}/requirements.txt'.format(devdock_root, service_path))

        # Copy wheels directory for custom packages (remove existing to avoid nested wheels/wheels)
        local('rm -rf {}/wheels && cp -r {}/python_scio/requirements/wheels {}/wheels'.format(service_path, devdock_root, service_path))

        dockerfile_template = "{}/devdock/core/templates/Dockerfile.python.tpl".format(devdock_root)
    elif service_type == "go":
        dockerfile_template = "{}/devdock/core/templates/Dockerfile.go.tpl".format(devdock_root)
    else:
        dockerfile_template = "{}/devdock/core/templates/Dockerfile.tpl".format(devdock_root)

    # Set up Docker build
    docker_config = setup_docker_build(context, dockerfile=dockerfile_template)

    # First set up docker-compose
    docker_compose("{}/devdock/config/services/{}/docker-compose.yaml".format(devdock_root, name))

    # Enable log streaming for the resource
    export_logs(name, name)

    # Then set up resource with dependencies
    deps = config.get("dependencies", []) + ['init-gcloud-auth','{}-bazel-build'.format(name), 'log_exporter_{}'.format(name)]

    if profile.get('infra',{}).get('sql',{}).get('type', 'remote') == 'local':
        deps.append('sql-hydrator')

    # Add pre-service hooks as dependencies
    pre_hooks = get_pre_service_hooks(name)
    if pre_hooks:
        deps.extend(pre_hooks)
        print("Added {} pre-service hooks as dependencies for service '{}'".format(len(pre_hooks), name))

    resource = dc_resource(
        name,
        labels=["service"],
        resource_deps=deps,
        auto_init=auto_init,
    )

    # Add port forward if debug port is configured
    if debug_port:
        port_forward(int(debug_port), int(debug_port))

    return resource
