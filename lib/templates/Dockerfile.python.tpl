FROM $base_image

WORKDIR /app

# Copy wheels directory for custom packages
COPY wheels /app/wheels

# Copy core requirements.txt
COPY requirements.txt /app/requirements.txt

# Install Python dependencies with access to local wheels
# --no-build-isolation: Required because flatdict 4.0.1 has a broken setup.py that imports
# pkg_resources without declaring it as a build dependency. Without this flag, uv builds
# packages in isolated environments where pkg_resources is unavailable, causing builds to
# fail on Linux AMD64 (which lacks pre-built wheels). Mac ARM64 builds work without this
# flag because wheels are available. This relies on the base image having setuptools installed.
RUN uv pip install --no-build-isolation --find-links /app/wheels -r /app/requirements.txt

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
