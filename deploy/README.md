# Deploying TakoIA core-backend

TakoIA is a single Rust binary that serves the REST API (`/api/*`) and the built
frontend over HTTP on port **8080**. It needs **no external database** (embedded
SQLite) and runs in **demo mode** out of the box — agent runs complete with
offline canned content. Set `CLAUDE_MAX_TOKEN` (a `claude setup-token` value) to
use real Claude.

Health endpoint: `GET /api/health` → `{"status":"ok"}`.

> The repository root already ships a `Dockerfile`, `docker-compose.yml` and
> `Makefile` for the **quick start** (`docker compose up --build`). This
> `deploy/` directory holds **self-contained, production-oriented** artifacts for
> each common install method. They do not depend on the root compose file.

## Methods

| Method | What it is | Run it | Use when |
|---|---|---|---|
| [`docker/`](docker/) | Plain `docker build` + `docker run`. | `docker build -t takoia . && docker run -p 8080:8080 -e ADMIN_PASSWORD=REPLACE_ME -v takoia-data:/app/data takoia` | A single host, no orchestration, fastest path to a running container. |
| [`docker-compose/`](docker-compose/) | Self-contained Compose file that builds from the repo root, named volume + healthcheck. | `docker compose -f deploy/docker-compose/docker-compose.yml up --build -d` | One host, you want restart policy, a healthcheck and persistent state declared in one file. |
| [`kubernetes/`](kubernetes/) | Plain manifests (Namespace, Secret, PVC, Deployment, Service, Ingress) + kustomization. | `kubectl apply -k deploy/kubernetes` | You have a cluster and want probes, secrets, persistent volume and ingress. |
| [`ansible/`](ansible/) | Playbook that installs Docker on a Debian/Ubuntu host and runs the container. | `ansible-playbook -i deploy/ansible/inventory.ini deploy/ansible/playbook.yml` | You manage one or more remote VMs over SSH and want a repeatable, idempotent install. |
| [`cloud-init/`](cloud-init/) | `user-data` that installs Docker and starts the container on first boot via a systemd unit. | Pass `deploy/cloud-init/user-data.yaml` as the VM's user data at creation. | Provisioning a fresh cloud VM that should come up already running TakoIA. |

## Prerequisites (be honest)

- **Docker / docker-compose / cloud-init / ansible** build the image from the
  repo, which compiles the Rust release binary and the Bun/Svelte frontend.
  First build takes **several minutes** and needs network access + a few GB of
  disk. To skip the build, push the image once to a registry and reference
  `ghcr.io/takoia/core-backend:latest` (placeholder — replace with your own).
- **Kubernetes** manifests reference an **image** (`ghcr.io/takoia/core-backend:latest`).
  A cluster cannot `docker build` from your repo, so you must build and push the
  image to a registry your cluster can pull from first.
- Always set **`ADMIN_PASSWORD`** for a stable login (otherwise a random one is
  generated and printed in the logs on every boot).
- Set **`MASTER_KEY`** (`openssl rand -base64 32`) to keep encrypted connector
  credentials readable across restarts. If unset, the entrypoint generates an
  ephemeral one each boot (fine for demo, not for production).

## Secrets

Every file uses `REPLACE_ME` placeholders. **Never commit real secrets.** Provide
`MASTER_KEY` and `ADMIN_PASSWORD` through your platform's secret mechanism
(Kubernetes Secret, Ansible vault, CI secrets, cloud metadata).
