# Secret storage backends

TakoIA stores connector / AI-tool secrets (LLM API keys, SMTP credentials, …)
through a **pluggable backend**, chosen in **Settings → Secret storage** (admin
only). The default keeps secrets encrypted in the local database; you can instead
delegate to an external secret manager.

| Backend | Where the secret lives | What TakoIA stores |
|---|---|---|
| `local` (default) | encrypted in the DB with `MASTER_KEY` | the encrypted value |
| `vault` | HashiCorp Vault (KV v2) | locator + (encrypted) token |
| `azure` | Azure Key Vault | **locator only** (`vault_name`) |
| `gcp` | GCP Secret Manager | **locator only** (`project`) |
| `aws` | AWS Secrets Manager | **locator only** (`region`) |

## The core principle: TakoIA stores no cloud credentials

> The hard part of any secret manager is *"where do you store the secret that
> unlocks the secret store?"*. TakoIA's answer is: **nowhere.**

External backends are driven through the cloud CLI (`az`, `gcloud`, `aws`,
`vault`) — the same way the agent engine shells out to `claude` and `icm`. The
CLI authenticates using the **identity of the host** TakoIA runs on. TakoIA only
persists the **locator** (vault name, project, region); for Azure / GCP / AWS it
holds **zero credentials**, so there is nothing to leak.

How a secret flows when an external backend is active:

1. On write, the value is handed to the CLI through a `0600` temp file (never on
   the command line) and pushed to the vault.
2. The DB keeps only an **encrypted reference** `@ext/<name>` — not the value.
3. On read, TakoIA resolves the reference back through the CLI.

The plaintext therefore never touches the local database.

## The authentication hierarchy (most to least secure)

1. **Workload / managed identity** (TakoIA runs *on* the cloud) — nothing to
   store. ✅ recommended.
2. **Workload Identity Federation** (off-cloud, OIDC → short-lived tokens).
3. **Certificate-based service principal** (Azure) / short-lived credential.
4. ❌ **Long-lived secret / key file** (SP client secret, SA JSON key) — last
   resort only, injected via the container environment or a mounted volume,
   **never** stored inside TakoIA.

## Azure Key Vault — Managed Identity (no secret)

Run TakoIA on Azure compute (VM, Container Apps, AKS) with a **system- or
user-assigned Managed Identity**.

1. Grant the identity the **Key Vault Secrets User** role on the vault
   (read) — add **Key Vault Secrets Officer** if TakoIA should also write:
   ```bash
   az role assignment create \
     --assignee <identity-client-id> \
     --role "Key Vault Secrets Officer" \
     --scope $(az keyvault show -n <vault-name> --query id -o tsv)
   ```
2. Make the CLI adopt the identity once at container start (no secret):
   ```bash
   az login --identity
   ```
3. In TakoIA: backend `azure`, params `{ "vault_name": "<vault-name>" }`.

**Off Azure:** use Workload Identity Federation (`az login --federated-token`)
or a service principal with a **certificate** (`az login --service-principal -u
<app-id> --tenant <tenant> -p <cert.pem>`). Avoid client secrets.

## GCP Secret Manager — Workload Identity (no key file)

Run TakoIA on GCE / Cloud Run / GKE with an **attached service account** (or GKE
Workload Identity binding).

1. Grant the service account the least-privilege role:
   ```bash
   gcloud secrets add-iam-policy-binding <secret-or-project> \
     --member="serviceAccount:<sa-email>" \
     --role="roles/secretmanager.secretAccessor"
   ```
2. `gcloud` / Application Default Credentials pick up the attached identity from
   the **metadata server automatically** — no `gcloud auth login`, no JSON key.
3. In TakoIA: backend `gcp`, params `{ "project": "<project-id>" }`.

**Off GCP:** use Workload Identity Federation (OIDC from your IdP → short-lived
GCP tokens). **Do not** download a service-account JSON key — that is the
anti-pattern this whole design avoids.

## AWS Secrets Manager — instance role / IRSA

Run TakoIA on EC2 (instance profile) or EKS (IRSA). Attach a role with the
minimal policy:

```json
{ "Effect": "Allow",
  "Action": ["secretsmanager:GetSecretValue", "secretsmanager:PutSecretValue", "secretsmanager:CreateSecret"],
  "Resource": "arn:aws:secretsmanager:<region>:<account>:secret:takoia-*" }
```

The `aws` CLI uses the instance/IRSA role automatically. In TakoIA: backend
`aws`, params `{ "region": "<region>" }`. No access keys stored.

## HashiCorp Vault — short-lived auth

Vault is the one backend where TakoIA may hold a `token`, encrypted at rest with
`MASTER_KEY`. Prefer a **short-lived token** obtained from Vault's own auth:

- **Kubernetes auth** or **AppRole** issues a renewable, scoped token; inject it
  as `VAULT_TOKEN`/`VAULT_ADDR` in the environment and leave TakoIA's `token`
  param blank, or paste a periodic token.
- Scope the token's policy to the KV path TakoIA uses (e.g. `secret/data/takoia-*`).

In TakoIA: backend `vault`, params `{ "addr": "https://vault:8200", "mount": "secret", "token": "<optional>" }`.

## What is the minimal footprint?

- TakoIA needs the matching CLI installed **and** authenticated on the host.
- For Azure / GCP / AWS it stores **only a locator** — no credential is ever
  written to TakoIA's database or logs.
- Secret values are passed to CLIs via a `0600` temp file and removed right
  after; they never appear in process arguments.
- Grant the **least-privilege** role (read-only `secretAccessor` /
  `Secrets User` if TakoIA only reads; add write only if it should create
  secrets).

## Quick reference

| Cloud | Host identity | Minimal role | TakoIA params |
|---|---|---|---|
| Azure | Managed Identity | Key Vault Secrets User (+ Officer to write) | `vault_name` |
| GCP | Attached SA / Workload Identity | `roles/secretmanager.secretAccessor` | `project` |
| AWS | Instance profile / IRSA | `secretsmanager:GetSecretValue` (+ Put/Create) | `region` |
| Vault | AppRole / k8s auth token | policy on `secret/data/takoia-*` | `addr`, `mount`, `token?` |
