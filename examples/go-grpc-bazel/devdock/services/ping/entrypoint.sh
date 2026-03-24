#!/bin/sh
set -e

# Go services are mounted at /app/{service_name}/{binary_name}
# devdock mounts ~/.devdock/ping → /app/ping; binary inside is named "ping"
exec /app/ping/ping
