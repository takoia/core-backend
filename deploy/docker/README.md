# Deploy with plain Docker

Run TakoIA as a single container. No external database is required (embedded
SQLite). The image is built from the repository root `Dockerfile` (multi-stage:
Bun/Svelte frontend + Rust release binary on a slim Debian runtime).

## 1. Build the image

From the **repository root** (the directory that contains the `Dockerfile`):

```bash
docker build -t takoia .
```

The first build compiles the Rust binary and the frontend; it takes several
minutes and needs network access. Alternatively, pull a prebuilt image:

```bash
docker pull ghcr.io/takoia/core-backend:latest   # placeholder — use your registry
```

## 2. Run it

```bash
docker run -d \
  --name takoia \
  -p 8080:8080 \
  -e ADMIN_PASSWORD=REPLACE_ME \
  -v takoia-data:/app/data \
  takoia
```

Then open <http://localhost:8080> and log in as `admin` / `REPLACE_ME`.

What this does:

- `-p 8080:8080` — publishes the HTTP server (API + frontend).
- `-e ADMIN_PASSWORD=...` — stable admin login. Without it, a random password is
  generated on every boot and printed in the container logs (`docker logs takoia`).
- `-v takoia-data:/app/data` — persists the SQLite databases (`takoia.db`,
  `icm.db`) across restarts. The image already declares `/app/data` as a volume.

The image ships sane defaults (`BIND_ADDR=0.0.0.0:8080`, `ADMIN_USERNAME=admin`,
`DATABASE_URL`, `ICM_DB_PATH`, `AGENT_WORKDIR`), so no other variable is needed.

## Demo mode vs. real Claude

Out of the box TakoIA runs in **demo mode**: agent runs complete with offline
canned content, no LLM credentials needed. To use real Claude, pass a token from
`claude setup-token`:

```bash
docker run -d --name takoia -p 8080:8080 \
  -e ADMIN_PASSWORD=REPLACE_ME \
  -e CLAUDE_MAX_TOKEN=REPLACE_ME_claude_setup_token \
  -v takoia-data:/app/data \
  takoia
```

## Persist encrypted connectors (recommended for non-demo)

By default the entrypoint generates an **ephemeral `MASTER_KEY`** each boot, so
encrypted connector credentials become unreadable after a restart. To keep them,
set a stable key:

```bash
MASTER_KEY="$(openssl rand -base64 32)"   # store this safely!

docker run -d --name takoia -p 8080:8080 \
  -e ADMIN_PASSWORD=REPLACE_ME \
  -e MASTER_KEY="$MASTER_KEY" \
  -v takoia-data:/app/data \
  takoia
```

## Health check

```bash
curl http://localhost:8080/api/health
# {"status":"ok"}
```

## Logs / stop / remove

```bash
docker logs -f takoia
docker stop takoia
docker rm takoia
# data survives in the named volume until you run: docker volume rm takoia-data
```
