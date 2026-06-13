//! Skills catalog + install. Skills are Claude Code skill folders (a `SKILL.md`)
//! that extend what an agent can do. The catalog is curated with logos; skills
//! can also be discovered from any GitHub repo and installed into
//! `~/.claude/skills/<id>/`.

use crate::error::{AppError, AppResult};
use axum::extract::Query;
use axum::Json;
use serde::Deserialize;
use serde_json::{json, Value};
use std::path::PathBuf;

const CATALOG_JSON: &str = include_str!("../../assets/skills_catalog.json");
const USER_AGENT: &str = "takoia-core";

fn skills_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(home).join(".claude").join("skills")
}

/// Resolve a GitHub token to lift the unauthenticated rate limit: prefer
/// `GITHUB_TOKEN`, fall back to `gh auth token`.
async fn github_token() -> Option<String> {
    if let Ok(t) = std::env::var("GITHUB_TOKEN") {
        if !t.trim().is_empty() {
            return Some(t);
        }
    }
    let out = tokio::process::Command::new("gh")
        .arg("auth")
        .arg("token")
        .output()
        .await
        .ok()?;
    if out.status.success() {
        let t = String::from_utf8_lossy(&out.stdout).trim().to_string();
        if !t.is_empty() {
            return Some(t);
        }
    }
    None
}

/// `GET /api/skills/catalog` — the curated skills catalog.
pub async fn catalog() -> AppResult<Json<Value>> {
    let parsed: Value = serde_json::from_str(CATALOG_JSON)
        .map_err(|e| AppError::Other(anyhow::anyhow!("invalid catalog: {e}")))?;
    Ok(Json(json!({ "skills": parsed })))
}

/// `GET /api/skills/installed` — skill folders present under ~/.claude/skills.
pub async fn installed() -> AppResult<Json<Value>> {
    let dir = skills_dir();
    let mut names = Vec::new();
    if let Ok(mut rd) = tokio::fs::read_dir(&dir).await {
        while let Ok(Some(entry)) = rd.next_entry().await {
            if entry.path().join("SKILL.md").exists() {
                if let Some(n) = entry.file_name().to_str() {
                    names.push(n.to_string());
                }
            }
        }
    }
    names.sort();
    Ok(Json(json!({ "installed": names })))
}

#[derive(Deserialize)]
pub struct InstallSkill {
    /// Local skill id (folder name).
    pub id: String,
    /// GitHub repo "owner/name".
    pub repo: String,
    /// Path to the skill folder within the repo.
    #[serde(default)]
    pub path: String,
    #[serde(default = "default_branch")]
    pub branch: String,
}

fn default_branch() -> String {
    "main".into()
}

/// `POST /api/skills/install` — fetch a skill's SKILL.md from GitHub and write it
/// into ~/.claude/skills/<id>/.
pub async fn install(Json(body): Json<InstallSkill>) -> AppResult<Json<Value>> {
    let path = body.path.trim_matches('/');
    let raw_url = if path.is_empty() {
        format!(
            "https://raw.githubusercontent.com/{}/{}/SKILL.md",
            body.repo, body.branch
        )
    } else {
        format!(
            "https://raw.githubusercontent.com/{}/{}/{}/SKILL.md",
            body.repo, body.branch, path
        )
    };

    let client = reqwest::Client::new();
    let resp = client
        .get(&raw_url)
        .header("User-Agent", USER_AGENT)
        .send()
        .await
        .map_err(|e| AppError::Other(e.into()))?;
    if !resp.status().is_success() {
        return Err(AppError::NotFound(format!(
            "SKILL.md not found at {raw_url} ({})",
            resp.status()
        )));
    }
    let content = resp.text().await.map_err(|e| AppError::Other(e.into()))?;

    let target_dir = skills_dir().join(&body.id);
    tokio::fs::create_dir_all(&target_dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;
    tokio::fs::write(target_dir.join("SKILL.md"), content.as_bytes())
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    Ok(Json(json!({ "ok": true, "installed": body.id, "source": raw_url })))
}

#[derive(Deserialize)]
pub struct GithubQuery {
    /// "owner/name".
    pub repo: String,
    #[serde(default)]
    pub path: String,
}

/// `GET /api/skills/github?repo=owner/name&path=...` — list candidate skill
/// folders (directories containing a SKILL.md is assumed) in a GitHub repo.
pub async fn github(Query(q): Query<GithubQuery>) -> AppResult<Json<Value>> {
    let api_url = format!(
        "https://api.github.com/repos/{}/contents/{}",
        q.repo,
        q.path.trim_matches('/')
    );
    let client = reqwest::Client::new();
    let mut request = client
        .get(&api_url)
        .header("User-Agent", USER_AGENT)
        .header("Accept", "application/vnd.github+json");
    if let Some(token) = github_token().await {
        request = request.bearer_auth(token);
    }
    let resp = request.send().await.map_err(|e| AppError::Other(e.into()))?;
    if !resp.status().is_success() {
        return Err(AppError::NotFound(format!(
            "GitHub repo/path not found ({})",
            resp.status()
        )));
    }
    let items: Value = resp.json().await.map_err(|e| AppError::Other(e.into()))?;

    // Keep directories — each is a candidate skill folder.
    let folders: Vec<Value> = items
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter(|i| i.get("type").and_then(|t| t.as_str()) == Some("dir"))
                .map(|i| {
                    json!({
                        "name": i.get("name").and_then(|v| v.as_str()).unwrap_or(""),
                        "path": i.get("path").and_then(|v| v.as_str()).unwrap_or(""),
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(Json(json!({ "repo": q.repo, "folders": folders })))
}
