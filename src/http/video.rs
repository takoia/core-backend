//! Video analysis: the browser extracts one frame per second and posts them
//! here; we hand the frames to `claude -p` (vision) for analysis. Claude reads
//! images, so a video is analyzed as an ordered sequence of sampled frames.

use crate::error::{AppError, AppResult};
use crate::llm::claude_cli::{self, ClaudeRun, ClaudeRuntime, ToolSet};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use std::time::Duration;
use uuid::Uuid;

/// Hard cap on frames per request to bound cost and context size.
const MAX_FRAMES: usize = 40;

/// Hard wall-clock limit for the video analysis run (the child is killed on
/// expiry, see `claude_cli::run`).
const VIDEO_TIMEOUT: Duration = Duration::from_secs(180);

#[derive(Deserialize)]
pub struct AnalyzeVideo {
    /// Base64-encoded PNG/JPEG frames (one per second), in order. May be raw
    /// base64 or a `data:image/...;base64,` URL.
    pub frames: Vec<String>,
    /// Optional analysis instruction; a sensible default is used otherwise.
    #[serde(default)]
    pub prompt: Option<String>,
    /// Optional target agent: its ICM memory is recalled to inform the analysis
    /// and the result is stored back, so analyses build on each other.
    #[serde(default)]
    pub agent_id: Option<String>,
}

/// `POST /api/video/analyze` — analyze sampled video frames with claude -p.
pub async fn analyze(
    State(state): State<AppState>,
    crate::http::users::CurrentUser(me): crate::http::users::CurrentUser,
    Json(body): Json<AnalyzeVideo>,
) -> AppResult<Json<Value>> {
    // Each call spends the operator's plan on up to MAX_FRAMES images: attach it
    // to an agent the caller may edit, or be an admin.
    match &body.agent_id {
        Some(aid) => crate::http::users::require_agent_role(&state, aid, &me, "editor").await?,
        None => crate::http::users::require_admin(&me)?,
    }
    if body.frames.is_empty() {
        return Err(AppError::BadRequest("no frames provided".into()));
    }
    let frames: Vec<&String> = body.frames.iter().take(MAX_FRAMES).collect();

    // Write frames into an isolated per-request directory.
    let dir =
        std::path::Path::new(&state.config.agent_workdir).join(format!("video-{}", Uuid::new_v4()));
    tokio::fs::create_dir_all(&dir)
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    let mut paths = Vec::new();
    for (i, frame) in frames.iter().enumerate() {
        let raw = frame.split_once(',').map(|(_, b)| b).unwrap_or(frame);
        let bytes = STANDARD
            .decode(raw.trim())
            .map_err(|_| AppError::BadRequest(format!("frame {i} is not valid base64")))?;
        let path = dir.join(format!("frame_{i:03}.png"));
        tokio::fs::write(&path, &bytes)
            .await
            .map_err(|e| AppError::Other(e.into()))?;
        paths.push(path.to_string_lossy().to_string());
    }

    let instruction = body.prompt.unwrap_or_else(|| {
        "These images are frames sampled one per second, in order, from a screen \
         recording or video."
            .to_string()
    });

    // ICM recall: bring in what the target agent already learned from previous
    // analyses, so each video analysis builds on the prior ones.
    let recalled = match &body.agent_id {
        Some(aid) => state.memory.recall(aid, &instruction, 5).await,
        None => String::new(),
    };
    let memory_block = if recalled.trim().is_empty() {
        String::new()
    } else {
        format!("\n\nWhat this agent already knows (build on it, avoid repeats):\n{recalled}")
    };

    // Ask for structured, human-reviewable extracted information.
    let prompt = format!(
        "{instruction}{memory_block}\n\nRead the frames in order, then EXTRACT the distinct, \
         factual pieces of information observed (actions performed, on-screen data, \
         text, entities, steps). Return ONLY a JSON array, each item \
         {{\"info\": <short label>, \"detail\": <one-sentence detail>}}. No prose, \
         no code fences.\n\nFrames:\n{}",
        paths.join("\n")
    );

    // The frames directory is the run's workdir: the only place the sandboxed
    // child may read, and the tool set is Read only.
    let rt = ClaudeRuntime {
        binary: "claude".to_string(),
        token: state.config.claude_max_token.clone(),
        workdir: dir.to_string_lossy().to_string(),
        sandbox: crate::sandbox::active(&state.db)
            .await
            .with_passthrough(&state.config.agent_env_passthrough),
        steering: false,
    };
    let out = claude_cli::run(
        &rt,
        ClaudeRun {
            system: None,
            prompt: &prompt,
            model: None,
            tools: ToolSet::ReadOnly,
            timeout: VIDEO_TIMEOUT,
        },
    )
    .await;
    let _ = tokio::fs::remove_dir_all(&dir).await;
    let out = out.map_err(|e| AppError::Other(anyhow::anyhow!("AI analysis failed: {e}")))?;
    let analysis = out.result;
    let usage = json!({
        "input_tokens": out.usage.prompt_tokens,
        "output_tokens": out.usage.completion_tokens,
    });

    // Try to parse the result as a JSON array of extracted items; fall back to a
    // single free-text item so the human always has something to confirm.
    let items = extract_items(&analysis);

    // ICM store: persist this analysis to the agent's memory so the next
    // analysis recalls it (continuous learning across analyses).
    if let Some(aid) = &body.agent_id {
        let summary: String = items
            .iter()
            .filter_map(|it| {
                let info = it.get("info").and_then(|v| v.as_str()).unwrap_or("");
                let detail = it.get("detail").and_then(|v| v.as_str()).unwrap_or("");
                (!info.is_empty()).then(|| format!("{info}: {detail}"))
            })
            .collect::<Vec<_>>()
            .join("; ");
        if !summary.trim().is_empty() {
            if let Err(e) = state.memory.store(aid, "video-analysis", &summary).await {
                tracing::warn!(error = %e, "failed to store video analysis in memory");
            }
        }
    }

    Ok(Json(json!({
        "items": items,
        "raw": analysis,
        "frame_count": paths.len(),
        "usage": usage,
    })))
}

/// Parse the model output into a list of `{info, detail}` items, tolerating
/// surrounding prose or code fences.
fn extract_items(text: &str) -> Vec<Value> {
    let trimmed = text
        .trim()
        .trim_start_matches("```json")
        .trim_start_matches("```")
        .trim_end_matches("```");
    let slice = match (trimmed.find('['), trimmed.rfind(']')) {
        (Some(a), Some(b)) if b > a => &trimmed[a..=b],
        _ => trimmed,
    };
    if let Ok(Value::Array(arr)) = serde_json::from_str::<Value>(slice) {
        if !arr.is_empty() {
            return arr;
        }
    }
    vec![json!({ "info": "Analysis", "detail": text })]
}
