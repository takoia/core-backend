//! Inbound webhooks: `POST /api/webhooks/:event` is reachable without a session
//! (external systems call it), so every payload must be signed with the
//! receiving agent's secret. Unsigned or mis-signed payloads create no job.

use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::body::Bytes;
use axum::extract::{Path, State};
use axum::http::HeaderMap;
use axum::Json;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use uuid::Uuid;

/// Header carrying `sha256=<hex>`; the GitHub-style name is accepted too so
/// off-the-shelf senders work unchanged.
const SIGNATURE_HEADERS: [&str; 2] = ["x-takoia-signature", "x-hub-signature-256"];

/// Constant-time check of `sha256=<hex>` against HMAC-SHA256(secret, body).
/// Returns false for a missing prefix, non-hex digest, or wrong length — never
/// panics on attacker-controlled input.
pub fn verify_signature(secret: &str, body: &[u8], header: &str) -> bool {
    let Some(hex) = header.trim().strip_prefix("sha256=") else {
        return false;
    };
    let Ok(expected) = hex_decode(hex.trim()) else {
        return false;
    };
    let Ok(mut mac) = Hmac::<Sha256>::new_from_slice(secret.as_bytes()) else {
        return false;
    };
    mac.update(body);
    mac.verify_slice(&expected).is_ok()
}

fn hex_decode(s: &str) -> Result<Vec<u8>, ()> {
    if !s.len().is_multiple_of(2) || !s.is_ascii() {
        return Err(());
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).map_err(|_| ()))
        .collect()
}

fn signature_header(headers: &HeaderMap) -> Option<&str> {
    SIGNATURE_HEADERS
        .iter()
        .find_map(|h| headers.get(*h).and_then(|v| v.to_str().ok()))
}

/// `POST /api/webhooks/:event` — body is the payload (raw text or JSON). Every
/// agent listening on `event` (trigger_on) whose secret validates the
/// signature gets a job with the payload. 401 when no listener accepts it.
pub async fn receive(
    State(state): State<AppState>,
    Path(event): Path<String>,
    headers: HeaderMap,
    body: Bytes,
) -> AppResult<Json<Value>> {
    // Attempts are counted per event name before any work, signed or not, so a
    // flood of unsigned payloads costs one map lookup and nothing else.
    if !state.webhook_limiter.allow(&event) {
        return Err(AppError::TooManyRequests(
            "too many webhook deliveries for this event; retry later".into(),
        ));
    }
    let Some(signature) = signature_header(&headers) else {
        return Err(AppError::Unauthorized(
            "missing X-Takoia-Signature (sha256=<hex hmac of the raw body>)".into(),
        ));
    };

    let listeners: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id, name, webhook_secret FROM agents WHERE trigger_on = ?")
            .bind(&event)
            .fetch_all(&state.db)
            .await?;

    let payload = String::from_utf8_lossy(&body);
    let mut jobs = Vec::new();
    let mut rejected = 0usize;
    for (agent_id, agent_name, secret) in &listeners {
        let accepted = secret
            .as_deref()
            .map(|s| verify_signature(s, &body, signature))
            .unwrap_or(false);
        if !accepted {
            rejected += 1;
            tracing::warn!(agent_id, event, "webhook signature rejected");
            continue;
        }

        let objective_id = Uuid::new_v4().to_string();
        let job_id = Uuid::new_v4().to_string();
        let title = format!("Webhook: {event}");
        let prompt = format!(
            "An inbound '{event}' event was received. Process this payload and \
             return the requested result.\n\nPayload:\n{payload}"
        );

        let mut tx = state.db.begin().await?;
        sqlx::query(
            r#"INSERT INTO objectives (id, account_id, agent_id, title, prompt)
               SELECT ?, account_id, ?, ?, ? FROM agents WHERE id = ?"#,
        )
        .bind(&objective_id)
        .bind(agent_id)
        .bind(&title)
        .bind(&prompt)
        .bind(agent_id)
        .execute(&mut *tx)
        .await?;
        sqlx::query(
            r#"INSERT INTO jobs (id, objective_id, agent_id, status, trigger_event)
               VALUES (?, ?, ?, 'queued', ?)"#,
        )
        .bind(&job_id)
        .bind(&objective_id)
        .bind(agent_id)
        .bind(&event)
        .execute(&mut *tx)
        .await?;
        tx.commit().await?;

        state.events.publish(crate::agent::JobEvent::status(
            &job_id,
            "queued",
            "webhook trigger",
        ));
        jobs.push(json!({ "job_id": job_id, "agent": agent_name }));
    }

    if jobs.is_empty() && rejected > 0 {
        return Err(AppError::Unauthorized("webhook signature invalid".into()));
    }
    Ok(Json(
        json!({ "event": event, "triggered": jobs, "rejected": rejected }),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sign(secret: &str, body: &[u8]) -> String {
        let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
        mac.update(body);
        format!("sha256={:x}", mac.finalize().into_bytes())
    }

    #[test]
    fn accepts_a_correct_signature() {
        let body = br#"{"invoice": 42}"#;
        assert!(verify_signature("s3cret", body, &sign("s3cret", body)));
    }

    #[test]
    fn rejects_wrong_secret_body_prefix_and_garbage() {
        let body = b"payload";
        let good = sign("s3cret", body);
        assert!(!verify_signature("other", body, &good));
        assert!(!verify_signature("s3cret", b"tampered", &good));
        assert!(!verify_signature(
            "s3cret",
            body,
            good.trim_start_matches("sha256=")
        ));
        assert!(!verify_signature("s3cret", body, "sha256=zz"));
        assert!(!verify_signature("s3cret", body, "sha256=abc"));
        assert!(!verify_signature("s3cret", body, "sha256=é1"));
        assert!(!verify_signature("s3cret", body, ""));
    }
}
