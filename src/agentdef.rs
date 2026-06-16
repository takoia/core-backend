//! Declarative agents: an agent is a TOML file (no code). The marketplace is a
//! catalog of these files. Each declares the event that triggers it
//! (`[trigger].on`) and the events it emits at the end of a run (`emit`).
//! Wiring two agents together = matching one agent's `emit` to another's
//! `trigger.on`.

use crate::db::Db;
use crate::domain::StepType;
use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Full declarative definition parsed from TOML.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentDef {
    pub agent: AgentMeta,
    #[serde(default)]
    pub trigger: Option<Trigger>,
    /// Per-step configuration keyed by step name (analyse/decision/action/restitution).
    #[serde(default)]
    pub steps: HashMap<String, StepDef>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct AgentMeta {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub author: String,
    #[serde(default = "default_version")]
    pub version: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub expertise: String,
    /// Seed identity/voice injected into every step.
    #[serde(default)]
    pub persona: String,
    #[serde(default = "default_autonomy")]
    pub autonomy: String,
    /// Custom emoji/icon shown for the agent.
    #[serde(default)]
    pub icon: String,
    /// Events emitted at the end of a run (choreography). Lives in `[agent]` so
    /// it is unambiguous regardless of table ordering in the file.
    #[serde(default)]
    pub emit: Vec<String>,
    #[serde(default)]
    pub visibility: Option<String>,
    #[serde(default)]
    pub price_per_run_usd: Option<f64>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct Trigger {
    pub on: String,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
pub struct StepDef {
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub provider: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allowed_tools: Vec<String>,
    /// Per-tool parameters (e.g. symbol, discord_webhook).
    #[serde(default, skip_serializing_if = "std::collections::HashMap::is_empty")]
    pub tool_params: std::collections::HashMap<String, String>,
}

fn default_version() -> String {
    "0.1.0".into()
}
fn default_autonomy() -> String {
    "confirm_before_action".into()
}

/// Parse a TOML agent definition.
pub fn parse(toml_str: &str) -> Result<AgentDef> {
    let def: AgentDef = toml::from_str(toml_str).context("invalid agent TOML")?;
    if def.agent.id.trim().is_empty() {
        return Err(anyhow!("agent.id is required"));
    }
    // The id becomes a filesystem path component (the agent workdir). Restrict it
    // to a strict slug so it can never traverse outside the agent workspace.
    if !def
        .agent
        .id
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(anyhow!(
            "agent.id must contain only letters, digits, '-' or '_'"
        ));
    }
    if def.agent.name.trim().is_empty() {
        return Err(anyhow!("agent.name is required"));
    }
    Ok(def)
}

/// Import (upsert) a declarative agent into the database. The TOML `id` (a slug)
/// is used as the agent primary key, so re-importing updates in place.
pub async fn import(db: &Db, account_id: &str, toml_str: &str) -> Result<String> {
    let def = parse(toml_str)?;
    let id = def.agent.id.clone();
    let autonomy = if def.agent.autonomy == "full_auto" {
        "full_auto"
    } else {
        "confirm_before_action"
    };
    let visibility = def.agent.visibility.as_deref().unwrap_or("private");
    let emit_json = serde_json::to_string(&def.agent.emit).unwrap_or_else(|_| "[]".into());
    let trigger_on = def.trigger.as_ref().map(|t| t.on.clone());

    let mut tx = db.begin().await?;
    sqlx::query(
        r#"INSERT INTO agents
             (id, account_id, name, description, autonomy_level, expertise_domain,
              author, version, trigger_on, emit, definition_toml, visibility, price_per_run_usd, icon, persona)
           VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
           ON CONFLICT(id) DO UPDATE SET
             name = excluded.name,
             description = excluded.description,
             autonomy_level = excluded.autonomy_level,
             expertise_domain = excluded.expertise_domain,
             author = excluded.author,
             version = excluded.version,
             trigger_on = excluded.trigger_on,
             emit = excluded.emit,
             definition_toml = excluded.definition_toml,
             visibility = excluded.visibility,
             price_per_run_usd = excluded.price_per_run_usd,
             icon = excluded.icon,
             persona = excluded.persona,
             updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
    )
    .bind(&id)
    .bind(account_id)
    .bind(&def.agent.name)
    .bind(&def.agent.description)
    .bind(autonomy)
    .bind(&def.agent.expertise)
    .bind(&def.agent.author)
    .bind(&def.agent.version)
    .bind(&trigger_on)
    .bind(&emit_json)
    .bind(toml_str)
    .bind(visibility)
    .bind(def.agent.price_per_run_usd.unwrap_or(0.0))
    .bind(&def.agent.icon)
    .bind(&def.agent.persona)
    .execute(&mut *tx)
    .await?;

    for (pos, step) in StepType::ALL.iter().enumerate() {
        let sd = def.steps.get(step.as_str()).cloned().unwrap_or_default();
        let mut options = serde_json::Map::new();
        if let Some(p) = &sd.provider {
            options.insert("provider".into(), serde_json::json!(p));
        }
        if let Some(m) = &sd.model {
            options.insert("model".into(), serde_json::json!(m));
        }
        if !sd.allowed_tools.is_empty() {
            options.insert("allowed_tools".into(), serde_json::json!(sd.allowed_tools));
        }
        if !sd.tool_params.is_empty() {
            options.insert("tool_params".into(), serde_json::json!(sd.tool_params));
        }
        let options_json = serde_json::Value::Object(options).to_string();

        sqlx::query(
            r#"INSERT INTO agent_step_configs (id, agent_id, step_type, system_prompt, options, position)
               VALUES (?, ?, ?, ?, ?, ?)
               ON CONFLICT(agent_id, step_type) DO UPDATE SET
                 system_prompt = excluded.system_prompt,
                 options = excluded.options,
                 updated_at = strftime('%Y-%m-%dT%H:%M:%fZ','now')"#,
        )
        .bind(uuid::Uuid::new_v4().to_string())
        .bind(&id)
        .bind(step.as_str())
        .bind(&sd.system_prompt)
        .bind(&options_json)
        .bind(pos as i64)
        .execute(&mut *tx)
        .await?;
    }
    tx.commit().await?;
    Ok(id)
}

/// Rebuild the TOML definition for an agent from the database (source of truth).
pub async fn export(db: &Db, agent_id: &str) -> Result<String> {
    #[derive(sqlx::FromRow)]
    struct AgentRow {
        id: String,
        name: String,
        description: String,
        autonomy_level: String,
        expertise_domain: String,
        author: String,
        version: String,
        trigger_on: Option<String>,
        emit: String,
        visibility: String,
        price_per_run_usd: f64,
        icon: String,
        persona: String,
    }
    let a = sqlx::query_as::<_, AgentRow>(
        r#"SELECT id, name, description, autonomy_level, expertise_domain, author,
                  version, trigger_on, emit, visibility, price_per_run_usd, icon, persona
           FROM agents WHERE id = ?"#,
    )
    .bind(agent_id)
    .fetch_optional(db)
    .await?
    .ok_or_else(|| anyhow!("agent not found"))?;

    #[derive(sqlx::FromRow)]
    struct StepRow {
        step_type: String,
        system_prompt: String,
        options: String,
    }
    let step_rows = sqlx::query_as::<_, StepRow>(
        "SELECT step_type, system_prompt, options FROM agent_step_configs WHERE agent_id = ?",
    )
    .bind(agent_id)
    .fetch_all(db)
    .await?;

    let mut steps = HashMap::new();
    for r in step_rows {
        let opts: serde_json::Value =
            serde_json::from_str(&r.options).unwrap_or(serde_json::Value::Null);
        steps.insert(
            r.step_type,
            StepDef {
                system_prompt: r.system_prompt,
                provider: opts.get("provider").and_then(|v| v.as_str()).map(String::from),
                model: opts.get("model").and_then(|v| v.as_str()).map(String::from),
                allowed_tools: opts
                    .get("allowed_tools")
                    .and_then(|v| v.as_array())
                    .map(|a| a.iter().filter_map(|x| x.as_str().map(String::from)).collect())
                    .unwrap_or_default(),
                tool_params: opts
                    .get("tool_params")
                    .and_then(|v| v.as_object())
                    .map(|m| {
                        m.iter()
                            .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                            .collect()
                    })
                    .unwrap_or_default(),
            },
        );
    }

    let def = AgentDef {
        agent: AgentMeta {
            id: a.id,
            name: a.name,
            author: a.author,
            version: a.version,
            description: a.description,
            expertise: a.expertise_domain,
            persona: a.persona,
            autonomy: a.autonomy_level,
            icon: a.icon,
            emit: serde_json::from_str(&a.emit).unwrap_or_default(),
            visibility: Some(a.visibility),
            price_per_run_usd: Some(a.price_per_run_usd),
        },
        trigger: a.trigger_on.map(|on| Trigger { on }),
        steps,
    };
    toml::to_string_pretty(&def).context("failed to serialize agent TOML")
}
