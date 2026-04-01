#!/bin/bash
set -e

# Create symlink from runfiles config directory to mounted config directory
RUNFILES_CONFIG_DIR="/app/${TEST_NAME}/${TEST_NAME}.runfiles/_main/config"
mkdir -p "$(dirname $RUNFILES_CONFIG_DIR)"
rm -rf "$RUNFILES_CONFIG_DIR"
ln -s /config "$RUNFILES_CONFIG_DIR"

# Check if debug mode is enabled
if [ "$DEBUG_MODE" == "True" ]; then
    echo "Starting test suite in debug mode on port ${DEBUG_PORT:-6000}"
    python -m debugpy --listen 0.0.0.0:${DEBUG_PORT:-6000} --wait-for-client \
        /app/${TEST_NAME}/${TEST_NAME}.py ${PARAMS_STR}
else
    python /app/${TEST_NAME}/${TEST_NAME}.py ${PARAMS_STR}
fi
