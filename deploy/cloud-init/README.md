# Deploy with cloud-init

`user-data.yaml` is a cloud-init `#cloud-config` that, on a fresh Debian/Ubuntu
cloud VM, installs Docker and brings up TakoIA core-backend as a
**systemd-managed container** (`takoia.service`) that restarts on failure and
survives reboots.

## Before you use it

Edit `user-data.yaml` and replace:

- `ADMIN_PASSWORD=REPLACE_ME` — your stable admin password.
- `MASTER_KEY=...` — `openssl rand -base64 32` (or remove the line to use an
  ephemeral key each restart).
- (Optional) `CLAUDE_MAX_TOKEN=...` for real Claude instead of demo mode.
- `TAKOIA_IMAGE=...` in the unit — point at your registry if not using the
  `ghcr.io/takoia/core-backend:latest` placeholder.

The VM **pulls** the image, so it must be available in a registry the VM can
reach. (Building from source on boot would require the repo on the VM and several
minutes of compile time — pulling a prebuilt image is the supported path here.)

## Pass it to a VM

The exact flag depends on your cloud:

```bash
# Generic libvirt / cloud-init datasource: the file is the instance user-data.

# AWS EC2
aws ec2 run-instances --image-id ami-xxxx --instance-type t3.small \
  --user-data file://deploy/cloud-init/user-data.yaml ...

# Google Cloud
gcloud compute instances create takoia \
  --metadata-from-file user-data=deploy/cloud-init/user-data.yaml ...

# Hetzner Cloud
hcloud server create --name takoia --image debian-12 --type cx22 \
  --user-data-from-file deploy/cloud-init/user-data.yaml

# DigitalOcean: paste the file contents into the "User data" box, or:
doctl compute droplet create takoia --image debian-12-x64 --size s-1vcpu-1gb \
  --region nyc1 --user-data-file deploy/cloud-init/user-data.yaml
```

## After boot

Open `http://<vm-ip>:8080` and log in as `admin` / your password. Open port 8080
in the VM's firewall / security group.

Check status on the VM:

```bash
systemctl status takoia
journalctl -u takoia -f
cloud-init status --long        # confirm user-data ran
curl http://localhost:8080/api/health   # {"status":"ok"}
```

State persists in the `takoia-data` Docker volume across restarts and reboots.
