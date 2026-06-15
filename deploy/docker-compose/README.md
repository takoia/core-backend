# Deploy with Docker Compose

A self-contained Compose file (alternative to the root `docker-compose.yml`). It
builds the image from the repository root, publishes port 8080, persists state in
a named volume, and adds a `/api/health` healthcheck with a restart policy.

## Run

From anywhere in the repo:

```bash
docker compose -f deploy/docker-compose/docker-compose.yml up --build -d
```

Then open <http://localhost:8080> and log in as `admin` / the `ADMIN_PASSWORD`
you set in the file.

## Before you deploy

Edit `docker-compose.yml` and replace:

- `ADMIN_PASSWORD: REPLACE_ME` — your stable admin password.
- (Recommended) uncomment and set `MASTER_KEY` (`openssl rand -base64 32`) to keep
  encrypted connector credentials across restarts.
- (Optional) uncomment `CLAUDE_MAX_TOKEN` (a `claude setup-token` value) to use
  real Claude instead of demo mode.

You can also keep secrets out of the file with an `.env` next to it and
`${MASTER_KEY}` / `${ADMIN_PASSWORD}` interpolation, or `docker compose --env-file`.

## Useful commands

```bash
docker compose -f deploy/docker-compose/docker-compose.yml logs -f
docker compose -f deploy/docker-compose/docker-compose.yml ps        # shows health
docker compose -f deploy/docker-compose/docker-compose.yml down      # stop (keeps volume)
docker compose -f deploy/docker-compose/docker-compose.yml down -v   # stop + delete data
```

## Notes

- `BIND_ADDR=0.0.0.0:8080` is required inside the container so the published port
  works (the app defaults to `127.0.0.1` outside containers).
- The runtime image is a slim Debian without `curl`; the healthcheck therefore
  opens a raw TCP connection to `127.0.0.1:8080` and checks the `/api/health`
  response contains `status`.
