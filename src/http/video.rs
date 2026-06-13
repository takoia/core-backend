//! Video analysis: the browser extracts one frame per second and posts them
//! here; we hand the frames to `claude -p` (vision) for analysis. Claude reads
//! images, so a video is analyzed as an ordered sequence of sampled frames.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use base64::{engine::general_purpose::STANDARD, Engine as _};
use serde::Deserialize;
use serde_json::{json, Value};
use tokio::io::AsyncWriteExt;
use tokio::process::Command;
use uuid::Uuid;

/// Hard cap on frames per request to bound cost and context size.
const MAX_FRAMES: usize = 40;

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
    Json(body): Json<AnalyzeVideo>,
) -> AppResult<Json<Value>> {
    if body.frames.is_empty() {
        return Err(AppError::BadRequest("no frames provided".into()));
    }
    let frames: Vec<&String> = body.frames.iter().take(MAX_FRAMES).collect();

    // Write frames into an isolated per-request directory.
    let dir = std::path::Path::new(&state.config.agent_workdir).join(format!("video-{}", Uuid::new_v4()));
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

    let mut child = Command::new("claude")
        .arg("-p")
        .arg("--output-format")
        .arg("json")
        .arg("--strict-mcp-config")
        .arg("--allowedTools")
        .arg("Read")
        .current_dir(&dir)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| AppError::Other(anyhow::anyhow!("failed to spawn claude: {e}")))?;

    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(prompt.as_bytes()).await;
        drop(stdin);
    }
    let output = child
        .wait_with_output()
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    // Best-effort cleanup of the frame directory.
    let _ = tokio::fs::remove_dir_all(&dir).await;

    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(AppError::Other(anyhow::anyhow!(
            "claude -p failed: {}",
            stderr.chars().take(300).collect::<String>()
        )));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let parsed: Value = serde_json::from_str(stdout.trim())
        .map_err(|e| AppError::Other(anyhow::anyhow!("invalid claude output: {e}")))?;
    let analysis = parsed
        .get("result")
        .and_then(|v| v.as_str())
        .unwrap_or_default()
        .to_string();
    let usage = parsed.get("usage").cloned().unwrap_or(json!({}));

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
    let trimmed = text.trim().trim_start_matches("```json").trim_start_matches("```").trim_end_matches("```");
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
