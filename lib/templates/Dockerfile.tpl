FROM $base_image

WORKDIR /app

# Copy entrypoint script if specified
$entrypoint_copy

# Set environment variables
$env_vars

# Expose ports
EXPOSE $port
$debugport_expose

# Make entrypoint script executable
$entrypoint_chmod

$run_commands

# Set entrypoint
$entrypoint_cmd
