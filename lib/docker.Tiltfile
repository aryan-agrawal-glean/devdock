# -*- mode: Python -*-
"""Docker-related utilities for building and managing containers."""

load("../lib/bazel.Tiltfile", "bazel_build", "bazel_sourcefile_deps")
load("./utils.Tiltfile", "get_bazel_cmd", "get_bazel_flags")

def cleanup_generated_files(service_name):
    """Clean up generated Dockerfile and docker-compose.yaml"""
    local('rm -f services/%s/Dockerfile services/%s/docker-compose.yaml' % (service_name, service_name))

def generate_dockerfile(template_path, context, type = 'services'):
    """Generate a Dockerfile from a template"""
    template = str(read_file(template_path))

    # Build the content line by line
    lines = []
    for line in template.split('\n'):
        if '$' in line:
            for key, value in context.items():
                if '$' + key in line:
                    line = line.replace('$' + key, str(value))
        lines.append(line)

    rendered = '\n'.join(lines)
    local('cat > %s/devdock/config/%s/%s/Dockerfile << EOL\n%s\nEOL' % (os.getenv('DEVDOCK_ROOT'), type, context['service'], rendered))
    return rendered

def generate_compose(template_path, context, type = 'services'):
    """Generate a docker-compose.yaml from a template"""
    template = str(read_file(template_path))

    # Read profile to get architecture
    devdock_root = os.getenv('DEVDOCK_ROOT')
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    architecture = profile.get('architecture', 'unknown')

    # Build the content line by line
    lines = []
    for line in template.split('\n'):
        if '$' in line:
            for key, value in context.items():
                if '$' + key in line:
                    line = line.replace('$' + key, str(value))
        lines.append(line)

    # Only add metrics port if not on Linux and if custom metrics port is specified
    if architecture != 'linux' and context.get('metrics_port', 10000) != 10000:
        ports_index = lines.index('    ports:')
        lines.insert(ports_index + 1, "      - \"{}:8081\"".format(context.get('metrics_port')))

    rendered = '\n'.join(lines)
    local('cat > %s/devdock/config/%s/%s/docker-compose.yaml << EOL\n%s\nEOL' % (os.getenv('DEVDOCK_ROOT'),type, context['service'], rendered))
    return rendered

def setup_docker_build(context, dockerfile = None, compose_file = None):
    """Set up a Docker build with optional custom Dockerfile and compose file"""
    dockerfile = dockerfile or "core/templates/Dockerfile.tpl"
    compose_file = compose_file or "{}/devdock/core/templates/compose.yaml.tpl".format(os.getenv('DEVDOCK_ROOT'))

    # Read profile to get architecture
    devdock_root = os.getenv('DEVDOCK_ROOT')
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    architecture = profile.get('architecture', 'unknown')

    # Build bazel info command with conditional bazelrc
    bazel_info_cmd = ["bazel"]
    bazel_info_cmd.extend(get_bazel_flags())
    bazel_info_cmd.extend(["info", "bazel-bin"])

    # Get bazel-bin directory
    bazel_bin = str(local(' '.join(bazel_info_cmd), quiet=True)).strip()
    service_name = context["service"]

    # Create temp directory for the service
    local('mkdir -p ~/.devdock/{}'.format(service_name))
    local('mkdir -p ~/.devdock/{}/logs'.format(service_name))
    local('chmod -R 755 ~/.devdock/{} || true'.format(service_name))
    # Handle different service types
    if context.get("type") == "python":

        # Update binary mount to use temp directory
        context["mount_binary_path"] = "~/.devdock/{service_name}:/app/{service_name}".format(service_name=service_name)

    elif context.get("type") == "go":

        # Update binary mount to use temp directory
        context["mount_binary_path"] = "~/.devdock/{service_name}:/app/{service_name}".format(service_name=service_name)

    elif context.get("type") == "pipeline":
        local('chmod -R 755 ~/.devdock/{} || true'.format(service_name))
        context["mount_binary_path"] = context["mount_binary_path"]
    elif context.get("type") == "test_suite":
        local('chmod -R 755 ~/.devdock/{} || true'.format(service_name))
        context["mount_binary_path"] = context["mount_binary_path"]
    else:
        # For Java, keep existing behavior
        local('chmod -R 755 ~/.devdock/{} || true'.format(service_name))
        war_filename = context["binary"].split("/")[-1]
        # If a previous run left a directory placeholder (Docker creates dirs for missing bind-mount files), remove it
        local('[ -d ~/.devdock/{}/{} ] && rm -rf ~/.devdock/{}/{} || true'.format(service_name, war_filename, service_name, war_filename))
        # Pre-create placeholder file so Docker bind-mounts a file, not a directory
        local('[ -f ~/.devdock/{}/{} ] || touch ~/.devdock/{}/{}'.format(service_name, war_filename, service_name, war_filename))
        # Update binary mount to use temp directory
        context["mount_binary_path"] = "~/.devdock/{}/{}:{}".format(service_name, war_filename, context.get("mount_binary_path", "").split(":")[1])
        # context["binary_copy"] = "COPY {} /app/service.jar".format(context["binary"])

    if context.get("type") == "pipeline":
        docker_config = {
            "dockerfile": generate_dockerfile(dockerfile, context, type = 'pipelines'),
            "compose": generate_compose(compose_file, context, type = 'pipelines'),
        }
    elif context.get("type") == "test_suite":
        docker_config = {
            "dockerfile": generate_dockerfile(dockerfile, context, type = 'test_suite'),
            "compose": generate_compose(compose_file, context, type = 'test_suite'),
        }
    else:
        docker_config = {
            "dockerfile": generate_dockerfile(dockerfile, context),
            "compose": generate_compose(compose_file, context),
        }
    return docker_config

def setup_networks():
    """Ensure required Docker networks exist"""
    networks = ["devdock_network", "devdock_infra"]
    for network in networks:
        local("docker network inspect {} >/dev/null 2>&1 || docker network create {}".format(
            network,
            network,
        ))
