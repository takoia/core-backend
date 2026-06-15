//! Pluggable secret storage for connector / AI-tool secrets.
//!
//! The default `local` backend encrypts secrets at rest with the master key.
//! External backends — HashiCorp Vault, Azure Key Vault, GCP Secret Manager and
//! AWS Secrets Manager — are driven through their CLIs (`vault`, `az`,
//! `gcloud`, `aws`), mirroring how the agent engine already shells out to
//! `claude` and `icm`. The operator authenticates the CLI on the host (CLI
//! login / instance identity); TakoIA only needs the locator (vault name,
//! project, region, address) which lives in the encrypted `secret_backend` row.
//!
//! When an external backend is active, `connectors.encrypted_secret` holds an
//! encrypted reference (`@ext/<name>`) instead of the value; the value itself
//! lives in the vault. Secrets are passed to the CLIs through a 0600 temp file,
//! never on the command line.

use crate::crypto::Cipher;
use crate::db::Db;
use anyhow::{anyhow, Result};
use serde_json::{json, Value};
use tokio::process::Command;

/// Reference marker stored (encrypted) when the value lives in a vault.
const EXT_PREFIX: &str = "@ext/";

/// Supported backend identifiers.
pub const BACKENDS: [&str; 5] = ["local", "vault", "azure", "gcp", "aws"];

#[derive(Clone, Debug)]
pub struct BackendConfig {
    pub kind: String,
    pub params: Value,
}

/// Reads/writes connector secrets through the active backend. Borrows the app
/// cipher + db so call sites build it cheaply: `SecretManager::new(&state.cipher, &state.db)`.
pub struct SecretManager<'a> {
    cipher: &'a Cipher,
    db: &'a Db,
}

impl<'a> SecretManager<'a> {
    pub fn new(cipher: &'a Cipher, db: &'a Db) -> Self {
        Self { cipher, db }
    }

    /// The active backend config (singleton row), defaulting to `local`.
    pub async fn active(&self) -> BackendConfig {
        let row: Option<(String, Option<Vec<u8>>)> =
            sqlx::query_as("SELECT kind, params FROM secret_backend WHERE id = 1")
                .fetch_optional(self.db)
                .await
                .ok()
                .flatten();
        match row {
            Some((kind, params_blob)) => {
                let params = params_blob
                    .filter(|b| !b.is_empty())
                    .and_then(|b| self.cipher.decrypt(&b).ok())
                    .and_then(|s| serde_json::from_str(&s).ok())
                    .unwrap_or_else(|| json!({}));
                BackendConfig { kind, params }
            }
            None => BackendConfig {
                kind: "local".into(),
                params: json!({}),
            },
        }
    }

    /// Persist the active backend selection + its config. Sensitive params left
    /// empty are preserved from the existing config (write-once secrets).
    pub async fn set_active(&self, kind: &str, params: &Value) -> Result<()> {
        if !BACKENDS.contains(&kind) {
            return Err(anyhow!("unknown secret backend: {kind}"));
        }
        let mut merged = params.clone();
        let existing = self.active().await;
        if let (Some(obj), Some(old)) = (merged.as_object_mut(), existing.params.as_object()) {
            for key in SENSITIVE_KEYS {
                let empty = obj
                    .get(key)
                    .and_then(|v| v.as_str())
                    .map(|s| s.trim().is_empty())
                    .unwrap_or(true);
                if empty {
                    if let Some(prev) = old.get(key) {
                        obj.insert(key.to_string(), prev.clone());
                    }
                }
            }
        }
        let blob = self.cipher.encrypt(&serde_json::to_string(&merged)?)?;
        sqlx::query(
            "UPDATE secret_backend SET kind = ?, params = ?, updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now') WHERE id = 1",
        )
        .bind(kind)
        .bind(blob)
        .execute(self.db)
        .await?;
        Ok(())
    }

    /// Store a connector secret, returning the bytes for `encrypted_secret`.
    /// Local → the encrypted value. External → pushed to the backend, with an
    /// encrypted `@ext/<name>` reference returned so the column stays opaque.
    pub async fn store_secret(&self, scope: &str, value: &str) -> Result<Vec<u8>> {
        let cfg = self.active().await;
        if cfg.kind == "local" {
            return self.cipher.encrypt(value);
        }
        let name = secret_name(scope);
        backend_set(&cfg, &name, value).await?;
        self.cipher.encrypt(&format!("{EXT_PREFIX}{name}"))
    }

    /// Resolve a stored `encrypted_secret` blob back to the plaintext secret.
    pub async fn resolve_blob(&self, blob: &[u8]) -> Result<String> {
        let plain = self.cipher.decrypt(blob)?;
        match plain.strip_prefix(EXT_PREFIX) {
            Some(name) => backend_get(&self.active().await, name).await,
            None => Ok(plain),
        }
    }

    /// Probe connectivity for a candidate backend config.
    pub async fn test(&self, cfg: &BackendConfig) -> Result<String> {
        backend_test(cfg).await
    }
}

/// Param keys whose values are secrets (masked in API responses, preserved on
/// empty update).
pub const SENSITIVE_KEYS: [&str; 3] = ["token", "secret_key", "access_key"];

fn secret_name(scope: &str) -> String {
    // Vault / Azure / GCP / AWS all accept [A-Za-z0-9-]; uuids already match.
    let safe: String = scope
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '-' })
        .collect();
    format!("takoia-{safe}")
}

fn param<'a>(cfg: &'a BackendConfig, key: &str) -> &'a str {
    cfg.params.get(key).and_then(|v| v.as_str()).unwrap_or("")
}

fn vault_env(c: &mut Command, cfg: &BackendConfig) {
    let addr = param(cfg, "addr");
    if !addr.is_empty() {
        c.env("VAULT_ADDR", addr);
    }
    let token = param(cfg, "token");
    if !token.is_empty() {
        c.env("VAULT_TOKEN", token);
    }
}

/// Write a secret to a fresh 0600 temp file so it never appears on a command line.
fn write_temp_secret(value: &str) -> Result<std::path::PathBuf> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    use rand::RngCore;
    let mut rnd = [0u8; 8];
    rand::thread_rng().fill_bytes(&mut rnd);
    let mut path = std::env::temp_dir();
    path.push(format!("takoia-secret-{:016x}", u64::from_le_bytes(rnd)));
    let mut f = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&path)?;
    f.write_all(value.as_bytes())?;
    Ok(path)
}

async fn run(cmd: &mut Command) -> Result<String> {
    let out = cmd.output().await?;
    if !out.status.success() {
        return Err(anyhow!(
            "{}",
            String::from_utf8_lossy(&out.stderr)
                .trim()
                .chars()
                .take(300)
                .collect::<String>()
        ));
    }
    Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
}

async fn backend_set(cfg: &BackendConfig, name: &str, value: &str) -> Result<()> {
    let path = write_temp_secret(value)?;
    let p = path.to_string_lossy().to_string();
    let res = match cfg.kind.as_str() {
        "vault" => {
            let mount = non_empty(param(cfg, "mount"), "secret");
            let mut c = Command::new("vault");
            vault_env(&mut c, cfg);
            c.args(["kv", "put", &format!("-mount={mount}"), name, &format!("value=@{p}")]);
            run(&mut c).await.map(|_| ())
        }
        "azure" => {
            let mut c = Command::new("az");
            c.args([
                "keyvault", "secret", "set",
                "--vault-name", param(cfg, "vault_name"),
                "--name", name,
                "--file", &p,
                "--output", "none",
            ]);
            run(&mut c).await.map(|_| ())
        }
        "gcp" => {
            let project = param(cfg, "project");
            let mut add = Command::new("gcloud");
            add.args(["secrets", "versions", "add", name, "--data-file", &p, "--project", project, "--quiet"]);
            if run(&mut add).await.is_err() {
                let mut create = Command::new("gcloud");
                create.args(["secrets", "create", name, "--data-file", &p, "--project", project, "--quiet"]);
                run(&mut create).await.map(|_| ())
            } else {
                Ok(())
            }
        }
        "aws" => {
            let region = param(cfg, "region");
            let secret_arg = format!("file://{p}");
            let mut put = Command::new("aws");
            put.args(["secretsmanager", "put-secret-value", "--secret-id", name, "--secret-string", &secret_arg, "--region", region, "--output", "text"]);
            if run(&mut put).await.is_err() {
                let mut create = Command::new("aws");
                create.args(["secretsmanager", "create-secret", "--name", name, "--secret-string", &secret_arg, "--region", region, "--output", "text"]);
                run(&mut create).await.map(|_| ())
            } else {
                Ok(())
            }
        }
        other => Err(anyhow!("unsupported backend: {other}")),
    };
    let _ = std::fs::remove_file(&path);
    res
}

async fn backend_get(cfg: &BackendConfig, name: &str) -> Result<String> {
    match cfg.kind.as_str() {
        "vault" => {
            let mount = non_empty(param(cfg, "mount"), "secret");
            let mut c = Command::new("vault");
            vault_env(&mut c, cfg);
            c.args(["kv", "get", &format!("-mount={mount}"), "-field=value", name]);
            run(&mut c).await
        }
        "azure" => {
            let mut c = Command::new("az");
            c.args([
                "keyvault", "secret", "show",
                "--vault-name", param(cfg, "vault_name"),
                "--name", name,
                "--query", "value",
                "--output", "tsv",
            ]);
            run(&mut c).await
        }
        "gcp" => {
            let mut c = Command::new("gcloud");
            c.args(["secrets", "versions", "access", "latest", "--secret", name, "--project", param(cfg, "project"), "--quiet"]);
            run(&mut c).await
        }
        "aws" => {
            let mut c = Command::new("aws");
            c.args(["secretsmanager", "get-secret-value", "--secret-id", name, "--query", "SecretString", "--output", "text", "--region", param(cfg, "region")]);
            run(&mut c).await
        }
        other => Err(anyhow!("unsupported backend: {other}")),
    }
}

async fn backend_test(cfg: &BackendConfig) -> Result<String> {
    match cfg.kind.as_str() {
        "local" => Ok("local encrypted storage".into()),
        "vault" => {
            let mut c = Command::new("vault");
            vault_env(&mut c, cfg);
            c.args(["status", "-format=json"]);
            run(&mut c).await.map(|_| "vault reachable".into())
        }
        "azure" => {
            let v = param(cfg, "vault_name");
            let mut c = Command::new("az");
            c.args(["keyvault", "secret", "list", "--vault-name", v, "--maxresults", "1", "--output", "none"]);
            run(&mut c).await.map(|_| format!("azure key vault '{v}' reachable"))
        }
        "gcp" => {
            let p = param(cfg, "project");
            let mut c = Command::new("gcloud");
            c.args(["secrets", "list", "--project", p, "--limit", "1", "--quiet"]);
            run(&mut c).await.map(|_| format!("gcp project '{p}' reachable"))
        }
        "aws" => {
            let r = param(cfg, "region");
            let mut c = Command::new("aws");
            c.args(["secretsmanager", "list-secrets", "--max-results", "1", "--region", r, "--output", "text"]);
            run(&mut c).await.map(|_| format!("aws region '{r}' reachable"))
        }
        other => Err(anyhow!("unknown backend: {other}")),
    }
}

fn non_empty<'a>(v: &'a str, default: &'a str) -> &'a str {
    if v.trim().is_empty() {
        default
    } else {
        v
    }
}
