FROM $base_image

WORKDIR /app

# Copy wheels directory for optional local packages (may be empty)
COPY wheels /app/wheels

# Copy requirements file (may be empty for pure-Docker services)
COPY requirements.txt /app/requirements.txt

# Install Python dependencies (no-op if requirements.txt is empty)
RUN if [ -s /app/requirements.txt ]; then \
    pip install --no-cache-dir --find-links /app/wheels -r /app/requirements.txt; \
  fi

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
