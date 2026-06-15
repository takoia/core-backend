# Deploy with Ansible

`playbook.yml` installs Docker Engine on a Debian/Ubuntu host (official Docker apt
repo) and runs the TakoIA container with a persistent volume and the right
environment. It is idempotent — re-running converges to the desired state.

## Requirements

- Ansible on your control machine, with the `community.docker` collection:

  ```bash
  ansible-galaxy collection install community.docker
  ```

- One or more **Debian/Ubuntu** hosts reachable over SSH, with sudo.
- The playbook **pulls** `ghcr.io/takoia/core-backend:latest` (placeholder). Edit
  `takoia_image` in `playbook.yml` to point at your registry, or push the image
  there first (the cluster/host cannot build from your repo).

## Run

```bash
cp deploy/ansible/inventory.example.ini deploy/ansible/inventory.ini
# edit inventory.ini: host, SSH user, key

ansible-playbook -i deploy/ansible/inventory.ini deploy/ansible/playbook.yml \
  -e takoia_admin_password='REPLACE_ME' \
  -e takoia_master_key="$(openssl rand -base64 32)"
```

Open `http://<host>:8080` and log in as `admin` / your password.

## Variables

| Variable | Default | Notes |
|---|---|---|
| `takoia_image` | `ghcr.io/takoia/core-backend:latest` | Image to pull. |
| `takoia_http_port` | `8080` | Host port published to the container. |
| `takoia_admin_username` | `admin` | Login user. |
| `takoia_admin_password` | `REPLACE_ME` | Set this. Stable admin login. |
| `takoia_master_key` | `""` | `openssl rand -base64 32`. Empty => ephemeral per restart. |
| `takoia_claude_max_token` | `""` | `claude setup-token` value. Empty => demo mode. |

For real deployments keep secrets in `ansible-vault` rather than on the CLI.

## What it does

1. Adds the Docker apt repo and installs Docker Engine + Compose plugin.
2. Ensures the Docker service is enabled and running.
3. Creates the `takoia-data` named volume.
4. Pulls the image and (re)runs the `takoia` container with
   `BIND_ADDR=0.0.0.0:8080`, the admin env, and the volume mounted at `/app/data`.
5. Waits for `GET /api/health` to return 200.
