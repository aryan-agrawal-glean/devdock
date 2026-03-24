#!/bin/sh
set -e

# Go services are mounted at /app/{service_name}/{binary_name}
# devdock mounts ~/.devdock/app → /app/app; binary inside is named "buildbuddy"
exec /app/app/buildbuddy
