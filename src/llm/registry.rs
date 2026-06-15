//! Provider registry: loads LLM connectors from the database (decrypting their
//! API keys) and resolves a provider by name, always with the canned provider
//! available as an offline fallback.

use super::{
    CannedProvider, ClaudeCliProvider, LlmProvider, OpenAiCompatProvider, CLAUDE_CLI_SENTINEL,
};
use crate::crypto::Cipher;
use crate::db::Db;
use anyhow::Result;
use std::collections::HashMap;
use std::sync::Arc;

/// A resolved set of providers for one account.
pub struct ProviderRegistry {
    providers: HashMap<String, Arc<dyn LlmProvider>>,
    canned: Arc<dyn LlmProvider>,
    default_name: String,
}

#[derive(sqlx::FromRow)]
struct LlmRow {
    name: String,
    base_url: String,
    model: String,
    encrypted_secret: Option<Vec<u8>>,
    is_default: i64,
}

impl ProviderRegistry {
    /// Load all `kind='llm'` connectors for an account and build providers.
    pub async fn load(
        db: &Db,
        cipher: &Cipher,
        account_id: &str,
        config_default: &str,
        agent_workdir: &str,
    ) -> Result<Self> {
        let rows = sqlx::query_as::<_, LlmRow>(
            r#"SELECT name, base_url, model, encrypted_secret, is_default
               FROM connectors WHERE account_id = ? AND kind = 'llm'"#,
        )
        .bind(account_id)
        .fetch_all(db)
        .await?;

        let mut providers: HashMap<String, Arc<dyn LlmProvider>> = HashMap::new();
        let mut default_name = config_default.to_string();

        // Resolve secrets through the active backend (local cipher decrypt, or an
        // external vault when the stored blob is an `@ext/<name>` reference).
        let secrets = crate::secrets::SecretManager::new(cipher, db);
        for row in rows {
            let secret = match row.encrypted_secret {
                Some(blob) if !blob.is_empty() => Some(secrets.resolve_blob(&blob).await?),
                _ => None,
            };

            // `base_url == "claude-cli"` selects the Claude Code headless
            // transport (consumes the plan via `claude -p`); anything else is a
            // standard OpenAI-compatible HTTP endpoint.
            let provider: Arc<dyn LlmProvider> = if row.base_url.trim() == CLAUDE_CLI_SENTINEL {
                Arc::new(ClaudeCliProvider::new(
                    row.name.clone(),
                    Some(row.model),
                    secret,
                    Some(agent_workdir.to_string()),
                ))
            } else {
                Arc::new(OpenAiCompatProvider::new(
                    row.name.clone(),
                    row.base_url,
                    secret,
                    row.model,
                ))
            };

            if row.is_default != 0 {
                default_name = row.name.clone();
            }
            providers.insert(row.name.clone(), provider);
        }

        let canned: Arc<dyn LlmProvider> = Arc::new(CannedProvider::new());
        Ok(Self { providers, canned, default_name })
    }

    /// Resolve a provider by name, falling back to the default, then to canned.
    pub fn resolve(&self, name: Option<&str>) -> Arc<dyn LlmProvider> {
        let wanted = name.unwrap_or(&self.default_name);
        if let Some(p) = self.providers.get(wanted) {
            return p.clone();
        }
        if let Some(p) = self.providers.get(&self.default_name) {
            return p.clone();
        }
        self.canned.clone()
    }

    /// The canned offline provider (used as a last-resort fallback on error).
    pub fn canned(&self) -> Arc<dyn LlmProvider> {
        self.canned.clone()
    }

    /// Names of all configured (non-canned) providers.
    pub fn names(&self) -> Vec<String> {
        self.providers.keys().cloned().collect()
    }
}
