#!/bin/sh
set -e

# Project root is mounted at /app via extra_mounts
# Adjust the path below to match your ingester's entry point
exec python /app/services/ingester/main.py
