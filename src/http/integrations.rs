//! External API integrations: configurable encrypted connectors (kind='integration').
//!
//! Most integrations (WhatsApp, SMS/Twilio, Google Cloud, Azure) only store
//! credentials here and require paid/verified accounts to actually send; their
//! send paths are best-effort stubs returning a clear "configure credentials"
//! message. Email (SMTP) is the one fully-functional example, sending via the
//! `lettre` crate.

use crate::bootstrap::DEFAULT_ACCOUNT_ID;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use axum::extract::State;
use axum::Json;
use lettre::message::header::ContentType;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncSmtpTransport, AsyncTransport, Message, Tokio1Executor};
use serde::Deserialize;
use serde_json::{json, Value};

/// SMTP credentials stored (encrypted) in the `email` integration connector.
#[derive(Deserialize)]
struct SmtpSecret {
    host: String,
    #[serde(default = "default_port")]
    port: u16,
    user: String,
    password: String,
    /// Sender address; falls back to `user` when omitted.
    #[serde(default)]
    from: String,
}

fn default_port() -> u16 {
    587
}

#[derive(sqlx::FromRow)]
struct SecretRow {
    base_url: String,
    encrypted_secret: Option<Vec<u8>>,
}

#[derive(Deserialize)]
pub struct EmailTestRequest {
    pub to: String,
    #[serde(default)]
    pub subject: String,
    #[serde(default)]
    pub body: String,
}

/// `POST /api/integrations/email/test` — read the `email` integration
/// connector, decrypt its SMTP credentials and send a real test email.
pub async fn email_test(
    State(state): State<AppState>,
    Json(req): Json<EmailTestRequest>,
) -> AppResult<Json<Value>> {
    if req.to.trim().is_empty() {
        return Err(AppError::BadRequest("recipient 'to' is required".into()));
    }

    let row = sqlx::query_as::<_, SecretRow>(
        r#"SELECT base_url, encrypted_secret
           FROM connectors
           WHERE account_id = ? AND kind = 'integration' AND name = 'email'"#,
    )
    .bind(DEFAULT_ACCOUNT_ID)
    .fetch_optional(&state.db)
    .await?;

    let row = row.ok_or_else(|| {
        AppError::BadRequest(
            "email integration not configured — save SMTP credentials first".into(),
        )
    })?;

    let blob = row.encrypted_secret.filter(|b| !b.is_empty()).ok_or_else(|| {
        AppError::BadRequest("email integration has no stored credentials".into())
    })?;

    let plaintext = crate::secrets::SecretManager::new(&state.cipher, &state.db)
        .resolve_blob(&blob)
        .await
        .map_err(AppError::Other)?;
    let secret: SmtpSecret = serde_json::from_str(&plaintext).map_err(|e| {
        AppError::BadRequest(format!(
            "stored SMTP credentials are not valid JSON (expected host/port/user/password): {e}"
        ))
    })?;

    // `base_url` may override the host (kept in sync with the saved connector).
    let host = if row.base_url.trim().is_empty() {
        secret.host.clone()
    } else {
        row.base_url.clone()
    };
    let from = if secret.from.trim().is_empty() {
        secret.user.clone()
    } else {
        secret.from.clone()
    };

    let subject = if req.subject.trim().is_empty() {
        "TakoIA test email".to_string()
    } else {
        req.subject.clone()
    };
    let body = if req.body.trim().is_empty() {
        "This is a test email sent by TakoIA to verify your SMTP integration.".to_string()
    } else {
        req.body.clone()
    };

    let email = Message::builder()
        .from(
            from.parse()
                .map_err(|e| AppError::BadRequest(format!("invalid sender address '{from}': {e}")))?,
        )
        .to(req
            .to
            .parse()
            .map_err(|e| AppError::BadRequest(format!("invalid recipient '{}': {e}", req.to)))?)
        .subject(subject)
        .header(ContentType::TEXT_PLAIN)
        .body(body)
        .map_err(|e| AppError::BadRequest(format!("failed to build message: {e}")))?;

    let creds = Credentials::new(secret.user.clone(), secret.password.clone());
    let mailer: AsyncSmtpTransport<Tokio1Executor> =
        AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&host)
            .map_err(|e| AppError::BadRequest(format!("invalid SMTP host '{host}': {e}")))?
            .port(secret.port)
            .credentials(creds)
            .build();

    match mailer.send(email).await {
        Ok(_) => Ok(Json(json!({
            "ok": true,
            "message": format!("test email sent to {}", req.to),
        }))),
        Err(e) => Err(AppError::Other(anyhow::anyhow!("SMTP send failed: {e}"))),
    }
}
