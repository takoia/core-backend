# Deploy on Kubernetes

Plain manifests (no Helm) for TakoIA core-backend: Namespace, Secret, PVC,
Deployment, Service and Ingress, tied together by a `kustomization.yaml`.

## Prerequisite: an image in a registry

A cluster cannot build from your repo. Build and push the image first:

```bash
# from the repository root
docker build -t ghcr.io/takoia/core-backend:latest .   # use YOUR registry
docker push ghcr.io/takoia/core-backend:latest
```

Then set that image reference in `deployment.yaml` (or via `kustomization.yaml`
`images:`).

## Set the secrets

Either edit `secret.yaml` (replace the `REPLACE_ME` values), or — recommended —
delete it and create the Secret imperatively so it never lands in git:

```bash
kubectl create namespace takoia
kubectl create secret generic takoia-secrets \
  --namespace takoia \
  --from-literal=MASTER_KEY="$(openssl rand -base64 32)" \
  --from-literal=ADMIN_PASSWORD="REPLACE_ME"
```

If you create the Secret imperatively, remove `secret.yaml` from
`kustomization.yaml`'s `resources:` list.

## Apply

```bash
kubectl apply -k deploy/kubernetes
```

Or without kustomize:

```bash
kubectl apply -f deploy/kubernetes/
```

## Verify

```bash
kubectl -n takoia get pods,svc,ingress,pvc
kubectl -n takoia logs deploy/takoia
# port-forward to test without an ingress:
kubectl -n takoia port-forward svc/takoia 8080:8080
curl http://localhost:8080/api/health   # {"status":"ok"}
```

## Notes

- **1 replica only.** TakoIA uses embedded SQLite (single writer) on a
  `ReadWriteOnce` PVC. The Deployment uses the `Recreate` strategy and must not
  be scaled up.
- Liveness/readiness probes hit `GET /api/health` on port 8080.
- Edit `ingress.yaml`: set your `host`, your `ingressClassName`, and uncomment the
  TLS block + cert-manager annotation once your issuer exists.
- Without `CLAUDE_MAX_TOKEN`, TakoIA runs in demo mode (offline canned content).
