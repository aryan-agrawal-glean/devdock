#!/bin/sh
set -e

# Project root is mounted at /app via extra_mounts
# Adjust the path below to match your processor's entry point
exec python /app/services/processor/main.py
