# syntax=docker/dockerfile:1
# TakoIA core-backend — one-command container.
# Builds the Svelte frontend + the Rust release binary into a slim runtime that
# needs NO external services. Without `claude`/`icm` it runs in demo mode:
# agent runs complete with offline canned content and SQLite-backed memory.
#
#   docker build -t takoia .
#   docker run -p 8080:8080 takoia      # open http://localhost:8080 (admin / takoia)

# ---- Stage 1: build the Svelte frontend bundle ----
FROM oven/bun:1 AS frontend
WORKDIR /app/frontend
COPY frontend/ ./
RUN bun install && bun run build

# ---- Stage 2: build the Rust release binary ----
FROM rust:1-bookworm AS backend
WORKDIR /app
COPY . .
RUN cargo build --release

# ---- Stage 3: minimal runtime ----
FROM debian:bookworm-slim AS runtime
RUN apt-get update \
 && apt-get install -y --no-install-recommends ca-certificates libssl3 \
 && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=backend /app/target/release/takoia /app/takoia
COPY --from=frontend /app/frontend/dist /app/frontend/dist
COPY docker-entrypoint.sh /app/docker-entrypoint.sh
RUN chmod +x /app/docker-entrypoint.sh && mkdir -p /app/data
# Sensible defaults so `docker run` works with zero configuration. Override any
# of these with `-e`. MASTER_KEY is generated at startup if unset (see entrypoint).
ENV BIND_ADDR=0.0.0.0:8080 \
    ADMIN_USERNAME=admin \
    ADMIN_PASSWORD=takoia \
    DATABASE_URL="sqlite:///app/data/takoia.db?mode=rwc" \
    ICM_DB_PATH=/app/data/icm.db \
    AGENT_WORKDIR=/tmp/takoia-agent-workspace \
    DEFAULT_LLM_PROVIDER=claude_max
EXPOSE 8080
VOLUME ["/app/data"]
ENTRYPOINT ["/app/docker-entrypoint.sh"]
