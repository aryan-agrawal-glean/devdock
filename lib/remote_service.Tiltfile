def setup_remote_service(name, port, remote_port = 80, namespace = '', service_suffix = True, override_remote_name = '', label = 'remote-service'):
    namespace_str, service_name = '', name
    if namespace != '':
        namespace_str = '-n {}'.format(namespace)

    if override_remote_name != '':
        service_name = override_remote_name

    if service_suffix:
        service_name = "{}-service".format(service_name)

    # Check if DNS kubectl is enabled from temp_profile
    devdock_root = os.getenv('DEVDOCK_ROOT')
    profile = read_yaml('{}/devdock/runtime_generated/temp_profile.yaml'.format(devdock_root))
    dns_kubectl_enabled = profile.get('dns_kubectl', 'disabled') == 'enabled'

    # Conditional kubectl verbosity based on DEBUG_MODE
    # v=1: Normal operations (default)
    # v=4: Debug level when DEBUG_MODE is set
    # v=8: Full HTTP request contents when KUBECTL_DEBUG_MODE is set
    kubectl_verbosity = '1'
    if os.getenv('KUBECTL_DEBUG_MODE', '').lower() in ('true', '1', 'yes'):
        kubectl_verbosity = '8'
    elif os.getenv('DEBUG_MODE', '').lower() in ('true', '1', 'yes'):
        kubectl_verbosity = '4'

    # Detect if we're on Linux and need to bind to Docker bridge IP
    # On Mac/Windows, Docker Desktop handles host.docker.internal automatically
    address_flag = ''
    if os.name != 'nt':  # Not Windows
        # Check if we're on Linux (not Mac/Darwin)
        uname_result = str(local('uname -s', quiet=True)).strip()
        if uname_result == 'Linux':
            # Get Docker bridge IP to allow container access
            docker_bridge_ip = str(local('ip -4 addr show docker0 2>/dev/null | grep -oP "(?<=inet\\s)\\d+(\\.\\d+){3}" || echo ""', quiet=True)).strip()
            if docker_bridge_ip:
                # Bind to both localhost and Docker bridge IP for security
                # This allows both local access and container access without exposing to public IP
                address_flag = '--address=localhost --address={}'.format(docker_bridge_ip)
            else:
                # Fallback to localhost only if docker0 not found
                print("Warning: Could not find docker0 interface, containers may not be able to connect to remote services")

    # Create a robust port-forward command with automatic retry logic
    # This handles cases where pods are terminated during deployments or node maintenance
    port_forward_cmd = '''
    set -euo pipefail

    # Configuration
    MAX_RETRIES=1000000  # Essentially infinite retries
    INITIAL_RETRY_DELAY=2
    MAX_RETRY_DELAY=30
    RETRY_DELAY=$INITIAL_RETRY_DELAY
    CONSECUTIVE_FAILURES=0
    MAX_CONSECUTIVE_FAILURES=10

    # Color codes for logging
    RED='\\033[0;31m'
    GREEN='\\033[0;32m'
    YELLOW='\\033[1;33m'
    NC='\\033[0m'  # No Color

    echo -e "${{GREEN}}[Port Forward Manager]${{NC}} Starting robust port-forward for {service_name} ({port}:{remote_port})"

    while true; do
        echo -e "${{GREEN}}[Port Forward Manager]${{NC}} Attempting to establish port-forward..."

        # Run kubectl port-forward and capture its exit code
        kubectl -v={kubectl_verbosity} port-forward {address_flag} service/{service_name} {port}:{remote_port} {namespace_str} && EXIT_CODE=$? || EXIT_CODE=$?

        if [ $EXIT_CODE -eq 0 ]; then
            # kubectl exited cleanly (unlikely for port-forward)
            echo -e "${{YELLOW}}[Port Forward Manager]${{NC}} Port-forward ended normally"
            CONSECUTIVE_FAILURES=0
            RETRY_DELAY=$INITIAL_RETRY_DELAY
        else
            # kubectl failed
            CONSECUTIVE_FAILURES=$((CONSECUTIVE_FAILURES + 1))

            if [ $CONSECUTIVE_FAILURES -ge $MAX_CONSECUTIVE_FAILURES ]; then
                echo -e "${{RED}}[Port Forward Manager]${{NC}} Failed $MAX_CONSECUTIVE_FAILURES times consecutively. There might be a configuration issue."
                echo -e "${{RED}}[Port Forward Manager]${{NC}} Common causes:"
                echo -e "${{RED}}[Port Forward Manager]${{NC}}   - Service {service_name} doesn't exist in the cluster"
                echo -e "${{RED}}[Port Forward Manager]${{NC}}   - No healthy pods backing the service"
                echo -e "${{RED}}[Port Forward Manager]${{NC}}   - Network/authentication issues"
                echo -e "${{YELLOW}}[Port Forward Manager]${{NC}} Will continue retrying with longer delays..."
            fi

            echo -e "${{YELLOW}}[Port Forward Manager]${{NC}} Port-forward failed with exit code $EXIT_CODE"
            echo -e "${{YELLOW}}[Port Forward Manager]${{NC}} This is often due to pod restarts during deployments"
            echo -e "${{YELLOW}}[Port Forward Manager]${{NC}} Retrying in $RETRY_DELAY seconds... (failure $CONSECUTIVE_FAILURES)"
        fi

        # Wait before retry
        sleep $RETRY_DELAY

        # Exponential backoff with max delay
        if [ $CONSECUTIVE_FAILURES -gt 0 ]; then
            RETRY_DELAY=$((RETRY_DELAY * 2))
            if [ $RETRY_DELAY -gt $MAX_RETRY_DELAY ]; then
                RETRY_DELAY=$MAX_RETRY_DELAY
            fi
        fi

        # Check if we should continue (allows for max retries if needed)
        if [ $MAX_RETRIES -ne 1000000 ] && [ $CONSECUTIVE_FAILURES -ge $MAX_RETRIES ]; then
            echo -e "${{RED}}[Port Forward Manager]${{NC}} Maximum retries ($MAX_RETRIES) reached. Exiting."
            exit 1
        fi
    done
    '''.format(
        kubectl_verbosity=kubectl_verbosity,
        address_flag=address_flag,
        service_name=service_name,
        port=port,
        remote_port=remote_port,
        namespace_str=namespace_str
    )

    # Configure serve_env and resource_deps based on DNS kubectl mode
    # When DNS kubectl is enabled, we don't need bastion proxy
    serve_env = {} if dns_kubectl_enabled else {'HTTPS_PROXY': 'localhost:7972'}
    resource_deps = [] if dns_kubectl_enabled else ['bastion']

    return local_resource(
         name,
         serve_cmd = ['bash', '-c', port_forward_cmd],
         readiness_probe=probe(
          initial_delay_secs=2,
          period_secs=5,
          exec=exec_action(
              command=["pgrep", "-f", "kubectl.*port-forward.*{}.*{}:{}".format(service_name, port, remote_port)]
          )
         ),
         serve_env=serve_env,
         labels=[label],
         allow_parallel=True,
         resource_deps=resource_deps
    )
