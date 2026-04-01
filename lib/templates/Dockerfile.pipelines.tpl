FROM $base_image

WORKDIR /app

# Copy the binary
# COPY $binary /app/service.jar

# Copy entrypoint script if specified
$entrypoint_copy

# Copy wrapper script if specified (DLP-specific)
$wrapper_script_copy

# Set environment variables
$env_vars

ENV GOOGLE_APPLICATION_CREDENTIALS=/tmp/.config/gcloud/application_default_credentials.json
ENV IS_DEVDOCK=true

# Expose ports
EXPOSE $port

# Make entrypoint script executable
$entrypoint_chmod

# Set entrypoint
$entrypoint_cmd
