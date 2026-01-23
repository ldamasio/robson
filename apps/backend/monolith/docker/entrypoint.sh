#!/bin/bash

# Exit immediately if a command exits with a non-zero status.
set -e

echo "🚀 Starting Robson Backend Entrypoint..."

# Run migrations
echo "⚙️ Applying database migrations..."
python manage.py migrate --noinput

# Start Gunicorn
echo "🏃 Starting Gunicorn server..."
exec gunicorn -b 0.0.0.0:8000 --worker-class=gevent --worker-connections=1000 --workers=5 backend.wsgi
