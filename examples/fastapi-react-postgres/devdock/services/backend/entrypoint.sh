#!/bin/sh
set -e

export PYTHONPATH=/app
cd /app

# Install the backend package and its dependencies from pyproject.toml
pip install --no-cache-dir -e . -q

# Run DB migrations then start the server
python -m app.backend_pre_start
alembic upgrade head
python -m app.initial_data
exec fastapi run --workers 1 app/main.py
