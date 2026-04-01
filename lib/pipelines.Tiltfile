"""Pipeline configuration and setup utilities."""

load("bazel.Tiltfile", "bazel_build")
load("docker.Tiltfile", "generate_compose", "setup_docker_build")
load('logger.Tiltfile', 'export_logs')

def expand_pipeline_env_var(value):
    """Expand environment variables in pipeline configuration values"""
    if type(value) != "string":  # In Starlark, type() returns a string
        return value

    if '${' not in value:
        return value

    # Handle ${VAR:-default} syntax
    if ':-' in value:
        start_idx = value.find('${')
        end_idx = value.find('}', start_idx)
        if start_idx != -1 and end_idx != -1:
            var_expr = value[start_idx+2:end_idx]  # Extract VAR:-default
            if ':-' in var_expr:
                var_name, default_val = var_expr.split(':-', 1)
                env_val = os.getenv(var_name.strip())
                if env_val:
                    return value.replace('${' + var_expr + '}', env_val)
                else:
                    return value.replace('${' + var_expr + '}', default_val)

    # Handle simple ${VAR} syntax
    start_idx = value.find('${')
    end_idx = value.find('}', start_idx)
    if start_idx != -1 and end_idx != -1:
        var_name = value[start_idx+2:end_idx]
        env_val = os.getenv(var_name)
        if env_val:
            return value.replace('${' + var_name + '}', env_val)

    return value

def load_pipeline_config(pipeline_name):
    """Load pipeline configuration from pipelines_config.yaml"""
    config_path = '{}/devdock/config/pipelines/pipelines_config.yaml'.format(os.getenv('DEVDOCK_ROOT'))
    if not os.path.exists(config_path):
        fail("Pipeline config not found at: " + config_path)

    config = read_yaml(config_path)
    if pipeline_name not in config:
        fail("Pipeline '{}' not found in config".format(pipeline_name))

    return config[pipeline_name]

def merge_pipeline_params(pipeline_config, profile_params):
    """Merge pipeline default params with profile-provided params"""
    merged_params = {}

    # Start with default params from pipeline config
    if "params" in pipeline_config:
        merged_params.update(pipeline_config["params"])

    # Add activity params
    if "type" in pipeline_config and pipeline_config["type"] == "activity":
        activity_params_path = '{}/devdock/config/pipelines/activity_params.yaml'.format(os.getenv('DEVDOCK_ROOT'))
        if os.path.exists(activity_params_path):
          activity_params = read_yaml(activity_params_path)
          merged_params.update(activity_params)
        else:
          fail("Activity params config not found at: " + activity_params_path)

    # Override with profile params
    if profile_params:
        merged_params.update(profile_params)

    return merged_params

def setup_pipeline(name, profile, project):
    """Set up a pipeline execution environment"""

    temp_profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(os.getenv('DEVDOCK_ROOT')))
    print("The profile is: {}".format(temp_profile))
    pipe_name = name
    if temp_profile.get('platform', '') == 'aws':
        pipe_name += '_aws'

    # Load the pipeline configuration
    pipeline_config = load_pipeline_config(pipe_name)

    # Get profile params if they exist
    profile_params = profile.get("params", {})

    # Merge params
    params = merge_pipeline_params(pipeline_config, profile_params)

    # Convert params to command line arguments, handling both flags and key-value pairs
    params_list = []
    if params:
        for key, val in params.items():
            # Expand environment variables in parameter values
            expanded_val = expand_pipeline_env_var(val) if val else val
            if expanded_val:
                params_list.append("--{}={}".format(key, expanded_val))
            else:
                params_list.append("--{}".format(key))
    params_str = " ".join(params_list)

    # Build the pipeline using Bazel
    # bazel_target must be specified in pipeline config
    bazel_target = pipeline_config.get('bazel_target', '')
    if not bazel_target:
        fail("Pipeline '{}' must specify 'bazel_target' in its pipeline config".format(pipe_name))

    watch_dirs = pipeline_config.get('watch_dirs', ["java"])

    # Run the Bazel build and get the binary path
    binary = bazel_build(
        name,
        bazel_target,
        watch_dirs,
        type="java",
        target_is_directory="False"
    )

    mount_binary_path = "~/.devdock/{}/dataflow_deploy.jar:/app/dataflow_deploy.jar".format(name, binary)
    if temp_profile.get('platform', '') == 'aws':
        mount_binary_path = "~/.devdock/{}/flink_deploy.jar:/app/flink_deploy.jar".format(name, binary)
    devdock_root = os.getenv('DEVDOCK_ROOT')

    # Create necessary directories and copy entrypoint script
    pipeline_dir = "{}/devdock/config/pipelines/{}".format(devdock_root, name)
    local("mkdir -p {}".format(pipeline_dir))
    # Copy platform-appropriate entrypoint script into the Docker build context
    tpl_dir = "{}/devdock/core/templates".format(devdock_root)
    entrypoint_src = "{}/entrypoint_pipelines.sh".format(tpl_dir)
    local("cp {} {}/entrypoint.sh".format(entrypoint_src, pipeline_dir))

    # Determine which main class to use based on pipeline execution method
    pipeline_execution = profile.get('pipelines', {}).get(name, {}).get('execution', 'auto')
    # For LocalBeamLauncher integration, 'dedicated'/'inline' execution uses direct pipeline main class
    # The LocalBeamLauncher will handle the execution via cron_helper triggers
    main_class = pipeline_config['main']
    if pipeline_execution == 'auto':
        print("Using auto execution method for {}: {}".format(name, main_class))
    elif pipeline_execution in ['dedicated', 'inline']:
        print("Using LocalBeamLauncher {} execution method for {}: {}".format(pipeline_execution, name, main_class))
    else:
        print("Using {} execution method for {}: {}".format(pipeline_execution, name, main_class))

    # Prepare environment variables for docker-compose
    env_vars = {
        "MAIN": main_class,
        "PIPELINE_EXECUTION": pipeline_execution,
        "PROJECT_NAME": project,
        "PROJECT_NUMBER": temp_profile.get('project_number', ''),
        "PROJECT_REGION": temp_profile.get('region', ''),
        "GOOGLE_CLOUD_PROJECT": project,
        "CLOUDSDK_AUTH_QUOTA_PROJECT": project,
        "PARAMS_STR": params_str,
        "DEBUG": profile.get("debug", "")  # Optional debug configuration
    }

    # Add pipeline-specific environment variables from profile
    pipeline_env = profile.get('env', {})
    if pipeline_env:
        for key, val in pipeline_env.items():
            expanded_val = expand_pipeline_env_var(val)
            env_vars[key] = expanded_val

    if temp_profile.get('platform', '') == 'aws':
        env_vars['AWS_REGION'] = temp_profile.get('region')
        env_vars['AWS_ACCOUNT_ID'] = temp_profile.get('aws_account_id')
        env_vars['AWS_ACCESS_KEY_ID'] = '${AWS_ACCESS_KEY_ID}'
        env_vars['AWS_SECRET_ACCESS_KEY'] = '${AWS_SECRET_ACCESS_KEY}'
        env_vars['AWS_SESSION_TOKEN'] = '${AWS_SESSION_TOKEN}'
        env_vars['RDS_SSL_CERT_PATH'] = '/tmp/global-bundle.pem'
        env_vars['IS_DEVDOCK'] = 'true'
        env_vars['IS_LOCAL_RUN'] = 'true'
        env_vars['CLASSPATH_LIMIT'] = '400000'
        env_vars['CLOUD_PLATFORM'] = 'AWS'
        env_vars['AWS_EC2_METADATA_DISABLED'] = 'true'
        env_vars['PIPELINE_CLASS_PATH'] = '/app/flink_deploy.jar'
        env_vars['PROJECT_NAME'] = temp_profile.get('aws_account_id')
        env_vars['GOOGLE_CLOUD_PROJECT'] = temp_profile.get('project')
        env_vars['CLOUDSDK_AUTH_QUOTA_PROJECT'] = temp_profile.get('project')

    # Format env vars for Dockerfile
    env_vars_str = "\n".join(['ENV {}="{}"'.format(k, v) for k, v in env_vars.items()])

    # Format env vars for docker-compose
    compose_env = "\n".join(['      {}: "{}"'.format(k, v) for k, v in env_vars.items()])

    # Set up Docker build context
    context = {
        "service": name,
        "base_image": "eclipse-temurin:17-jdk",
        "binary": binary,
        "mount_binary_path": mount_binary_path,
        "entrypoint_copy": "COPY entrypoint.sh /app/entrypoint.sh",
        "entrypoint_chmod": "RUN chmod +x /app/entrypoint.sh",
        "entrypoint_cmd": 'ENTRYPOINT ["/app/entrypoint.sh"]',
        "env_vars": env_vars_str,
        "compose_env": compose_env,
        "extra_mounts": "",
        "port": "9000",  # Default port
        "debugport_hosts": "",
        "healthcheck": "",
        "type": "pipeline",
        "dockerfile_template": "{}/devdock/core/templates/Dockerfile.pipelines.tpl".format(devdock_root),
        "compose_template": "{}/devdock/core/templates/compose.pipelines.yaml.tpl".format(devdock_root)
    }

    # Add custom executable if execution mode is dedicated container
    pipeline_config = profile.get('pipelines', {}).get(name, {})
    if pipeline_config.get('execution') == 'dedicated' and 'executable' in pipeline_config:
        executable = pipeline_config['executable']
        context["wrapper_script_copy"] = "COPY {} /app/{}\nRUN chmod +x /app/{}".format(executable, executable, executable)
    else:
        context["wrapper_script_copy"] = "# No custom executable specified"

    # Set up Docker build
    docker_config = setup_docker_build(context, compose_file = context['compose_template'], dockerfile = context['dockerfile_template'])

    # Set up docker-compose
    compose_file = "{}/docker-compose.yaml".format(pipeline_dir)
    docker_compose(compose_file)

    # Enable log streaming
    export_logs(name, name)

    # Set up resource with dependencies including the Bazel build
    resource = dc_resource(
        name,
        labels=["pipeline"],
        resource_deps=[
                          'init-gcloud-auth',
                          '{}-bazel-build'.format(name),
                          'log_exporter_{}'.format(name)
                      ] + profile.get('dependencies', []),
        auto_init=True
    )

    return resource
