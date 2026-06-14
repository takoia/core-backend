#!/bin/sh
# Generate an ephemeral MASTER_KEY if none was provided, so `docker run` works
# with zero configuration. Set MASTER_KEY explicitly (e.g. `openssl rand -base64
# 32`) to keep encrypted connector credentials readable across restarts.
set -e

if [ -z "$MASTER_KEY" ]; then
  MASTER_KEY="$(head -c 32 /dev/urandom | base64)"
  export MASTER_KEY
  echo "[entrypoint] generated an ephemeral MASTER_KEY (set MASTER_KEY to persist encrypted connectors across restarts)"
fi

echo "[entrypoint] starting TakoIA on ${BIND_ADDR:-0.0.0.0:8080} (admin user: ${ADMIN_USERNAME:-admin})"
exec /app/takoia
