#!/bin/sh
set -e

cd /app
npm install --silent
exec npm run dev -- --host 0.0.0.0 --port 5173
