//! Text-to-speech via OpenAI-compatible `audio/speech`. Reads the API key from
//! an encrypted connector (`openai_tts` preferred, else `codex`) and streams the
//! generated MP3 back to the browser.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::body::Body;
use axum::extract::State;
use axum::http::header;
use axum::response::Response;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct TtsInput {
    pub text: String,
    #[serde(default = "default_voice")]
    pub voice: String,
    #[serde(default = "default_model")]
    pub model: String,
}

fn default_voice() -> String {
    "alloy".into()
}
fn default_model() -> String {
    "gpt-4o-mini-tts".into()
}

#[derive(sqlx::FromRow)]
struct ProviderRow {
    base_url: String,
    encrypted_secret: Option<Vec<u8>>,
}

/// `POST /api/tts` — synthesize speech, returns `audio/mpeg`.
pub async fn synthesize(
    State(state): State<AppState>,
    axum::Json(body): axum::Json<TtsInput>,
) -> AppResult<Response> {
    if body.text.trim().is_empty() {
        return Err(AppError::BadRequest("text is required".into()));
    }

    let row = sqlx::query_as::<_, ProviderRow>(
        r#"SELECT base_url, encrypted_secret FROM connectors
           WHERE account_id = ? AND kind = 'llm'
             AND name IN ('openai_tts', 'codex')
             AND encrypted_secret IS NOT NULL
           ORDER BY CASE name WHEN 'openai_tts' THEN 0 ELSE 1 END
           LIMIT 1"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_optional(&state.db)
    .await?
    .ok_or_else(|| {
        AppError::BadRequest(
            "no OpenAI key configured — set the 'codex' (or 'openai_tts') provider key in Settings".into(),
        )
    })?;

    let key = match row.encrypted_secret {
        Some(blob) if !blob.is_empty() => state.cipher.decrypt(&blob).map_err(AppError::Other)?,
        _ => return Err(AppError::BadRequest("OpenAI provider has no key".into())),
    };
    let base_url = row.base_url.trim_end_matches('/');

    let resp = reqwest::Client::new()
        .post(format!("{base_url}/audio/speech"))
        .bearer_auth(key)
        .json(&serde_json::json!({
            "model": body.model,
            "voice": body.voice,
            "input": body.text,
            "response_format": "mp3",
        }))
        .send()
        .await
        .map_err(|e| AppError::Other(e.into()))?;

    if !resp.status().is_success() {
        let status = resp.status();
        let msg = resp.text().await.unwrap_or_default();
        return Err(AppError::Other(anyhow::anyhow!(
            "TTS provider returned {status}: {}",
            msg.chars().take(300).collect::<String>()
        )));
    }

    let audio = resp.bytes().await.map_err(|e| AppError::Other(e.into()))?;
    Response::builder()
        .header(header::CONTENT_TYPE, "audio/mpeg")
        .body(Body::from(audio))
        .map_err(|e| AppError::Other(e.into()))
}
