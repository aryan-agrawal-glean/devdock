#!/bin/sh
set -e

cd /app

# Install dependencies
pip install --no-cache-dir -r requirements.txt -q

# Start the Celery worker
cd /app/backend
exec celery -A backend worker --loglevel=info --concurrency 1 -E
