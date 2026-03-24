#!/bin/sh
set -e

cd /app

# Install dependencies (includes gunicorn not in requirements.txt)
pip install --no-cache-dir -r requirements.txt gunicorn -q

# Run Django migrations
cd /app/backend
python manage.py migrate --noinput

# Start the application server
exec gunicorn backend.wsgi --bind 0.0.0.0:8000 --workers 2
