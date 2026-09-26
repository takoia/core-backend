#!/bin/sh
set -e

# MASTER_KEY encrypts every stored credential. An ephemeral key means every
# connector becomes undecryptable at the next restart, so it is only acceptable
# for a throwaway demo container.
if [ -z "$MASTER_KEY" ]; then
  case "$(printf '%s' "${DEMO_MODE:-}" | tr '[:upper:]' '[:lower:]')" in
    1|true|yes)
      MASTER_KEY="$(head -c 32 /dev/urandom | base64)"
      export MASTER_KEY
      echo "[entrypoint] DEMO_MODE: generated an ephemeral MASTER_KEY (stored credentials will not survive a restart)"
      ;;
    *)
      echo "[entrypoint] MASTER_KEY is required (openssl rand -base64 32). Set DEMO_MODE=true to run a throwaway demo with an ephemeral key." >&2
      exit 1
      ;;
  esac
fi

echo "[entrypoint] starting TakoIA on ${BIND_ADDR:-0.0.0.0:8080} (admin user: ${ADMIN_USERNAME:-admin})"
exec /app/takoia
