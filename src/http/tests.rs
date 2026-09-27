//! Router-level tests: a real SQLite database (temp file), the real migrations,
//! the real router. They exercise the security boundary end to end — session
//! gate, per-agent RBAC, admin-only surfaces, webhook signatures — because a
//! unit test of one handler cannot prove that a route is actually protected.

use crate::config::Config;
use crate::state::AppState;
use axum::body::{to_bytes, Body};
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use hmac::{Hmac, Mac};
use serde_json::{json, Value};
use sha2::Sha256;
use tower::ServiceExt;

struct TestApp {
    router: Router,
    db: crate::db::Db,
    _dir: std::path::PathBuf,
}

async fn app() -> TestApp {
    // Never seed the showcase agent: tests want an empty catalogue.
    std::env::set_var("SEED_SHOWCASE_AGENT", "false");
    let dir = std::env::temp_dir().join(format!("takoia-test-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let db_url = format!("sqlite://{}/takoia.db?mode=rwc", dir.display());
    let pool = crate::db::connect(&db_url).await.unwrap();
    crate::db::migrate(&pool).await.unwrap();
    let config = Config {
        bind_addr: "127.0.0.1:0".into(),
        frontend_dev_origin: "http://localhost:5173".into(),
        database_url: db_url,
        master_key: [7u8; 32],
        default_llm_provider: "claude_max".into(),
        provider_seeds: vec![],
        icm_db_path: dir.join("icm.db").to_string_lossy().into_owned(),
        claude_max_token: None,
        agent_workdir: dir.join("ws").to_string_lossy().into_owned(),
        admin_username: "admin".into(),
        admin_password: None,
        memory_maintenance_interval_secs: 300,
        inner_life_interval_secs: 900,
        demo_mode: false,
        pricing: crate::pricing::Pricing::default(),
        sync_job_max_secs: 4 * 3600,
        invoke_max_output_tokens: 4096,
        marketplace_min_price_per_1k: 0.0,
        agent_env_passthrough: vec![],
    };
    let state = AppState::new(pool, config);
    crate::bootstrap::run(&state.db, &state.cipher, &state.config)
        .await
        .unwrap();
    TestApp {
        db: state.db.clone(),
        router: crate::http::router(state),
        _dir: dir,
    }
}

async fn call(
    app: &TestApp,
    method: Method,
    path: &str,
    token: Option<&str>,
    body: Option<Value>,
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(method).uri(path);
    if let Some(t) = token {
        req = req.header(header::AUTHORIZATION, format!("Bearer {t}"));
    }
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let req = match body {
        Some(v) => req
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(v.to_string()))
            .unwrap(),
        None => req.body(Body::empty()).unwrap(),
    };
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    let json = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, json)
}

async fn raw_post(
    app: &TestApp,
    path: &str,
    body: &[u8],
    extra: &[(&str, &str)],
) -> (StatusCode, Value) {
    let mut req = Request::builder().method(Method::POST).uri(path);
    for (k, v) in extra {
        req = req.header(*k, *v);
    }
    let req = req.body(Body::from(body.to_vec())).unwrap();
    let resp = app.router.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = to_bytes(resp.into_body(), 1 << 20).await.unwrap();
    (
        status,
        serde_json::from_slice(&bytes).unwrap_or(Value::Null),
    )
}

/// First-run setup: returns the admin's session token.
async fn setup_admin(app: &TestApp) -> String {
    let (status, v) = call(app, Method::GET, "/api/setup/status", None, None, &[]).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(v["needs_setup"], true);
    let (status, v) = call(
        app,
        Method::POST,
        "/api/setup",
        None,
        Some(json!({ "email": "admin@example.test", "password": "correct horse" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    v["token"].as_str().unwrap().to_string()
}

/// Admin creates a non-admin member and we log in as them.
async fn member(app: &TestApp, admin: &str, email: &str) -> (String, String) {
    let (status, v) = call(
        app,
        Method::POST,
        "/api/users",
        Some(admin),
        Some(json!({ "email": email, "password": "member pass", "is_admin": false })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    let user_id = v["id"].as_str().unwrap_or_default().to_string();
    let (status, v) = call(
        app,
        Method::POST,
        "/api/login",
        None,
        Some(json!({ "email": email, "password": "member pass" })),
        &[],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{v}");
    (user_id, v["token"].as_str().unwrap().to_string())
}

fn sign(secret: &str, body: &[u8]) -> String {
    let mut mac = Hmac::<Sha256>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    format!("sha256={:x}", mac.finalize().into_bytes())
}

#[tokio::test]
async fn session_gate_and_public_allow_list() {
    let app = app().await;
    let (s, _) = call(&app, Method::GET, "/api/agents", None, None, &[]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(
        &app,
        Method::GET,
        "/api/connectors",
        Some("not-a-token"),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(&app, Method::GET, "/api/health", None, None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(&app, Method::GET, "/api/marketplace", None, None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    // /v1 authenticates with a consumer key, not a session.
    let (s, _) = call(&app, Method::GET, "/api/v1/models", None, None, &[]).await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn rbac_matrix_for_a_member_without_roles() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (bob_id, bob) = member(&app, &admin, "bob@example.test").await;

    // Admin-only surfaces.
    for path in [
        "/api/connectors",
        "/api/memory/overview",
        "/api/logs",
        "/api/skills/installed",
        "/api/mcp/installed",
    ] {
        let (s, _) = call(&app, Method::GET, path, Some(&bob), None, &[]).await;
        assert_eq!(s, StatusCode::FORBIDDEN, "{path} must be admin-only");
    }
    let (s, _) = call(
        &app,
        Method::GET,
        "/api/connectors",
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    // Admin creates an agent; the member has no role on it.
    let (s, v) = call(
        &app,
        Method::POST,
        "/api/agents",
        Some(&admin),
        Some(json!({ "name": "Analyst" })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let agent = v["id"].as_str().unwrap().to_string();

    let (s, v) = call(&app, Method::GET, "/api/agents", Some(&bob), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(
        v["agents"].as_array().unwrap().len(),
        0,
        "lists are filtered to the caller's roles"
    );
    let (_, v) = call(&app, Method::GET, "/api/agents", Some(&admin), None, &[]).await;
    assert_eq!(
        v["agents"].as_array().unwrap().len(),
        1,
        "admins see everything"
    );

    let (s, _) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}/memories"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/schedules",
        Some(&bob),
        Some(json!({ "agent_id": agent, "prompt": "run", "interval_seconds": 60 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "scheduling another owner's agent");

    // Grant viewer: reads open up, writes and credentials stay closed.
    let (s, v) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/permissions"),
        Some(&admin),
        Some(json!({ "user_id": bob_id, "role": "viewer" })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let (s, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert!(
        v["webhook_secret"].is_null(),
        "viewers never see the webhook secret"
    );
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(
        v["webhook_secret"].as_str().map(str::len),
        Some(48),
        "owners do"
    );
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&bob),
        Some(json!({ "steps": [] })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "viewer cannot edit");
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/webhook-secret/rotate"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "viewer cannot rotate the secret");
}

#[tokio::test]
async fn inline_tool_secrets_are_masked_for_non_owners() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (bob_id, bob) = member(&app, &admin, "bob@example.test").await;
    let (_, v) = call(
        &app,
        Method::POST,
        "/api/agents",
        Some(&admin),
        Some(json!({ "name": "Alerter" })),
        &[],
    )
    .await;
    let agent = v["id"].as_str().unwrap().to_string();
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&admin),
        Some(json!({ "steps": [{
            "step_type": "action",
            "options": { "allowed_tools": ["send_discord"], "tool_params": { "discord_webhook": "https://discord.com/api/webhooks/1/topsecret" } }
        }] })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/permissions"),
        Some(&admin),
        Some(json!({ "user_id": bob_id, "role": "editor" })),
        &[],
    )
    .await;

    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    let action = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step_type"] == "action")
        .unwrap();
    assert!(
        !action["options"].as_str().unwrap().contains("topsecret"),
        "editor sees masked options"
    );
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    let action = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step_type"] == "action")
        .unwrap();
    assert!(
        action["options"].as_str().unwrap().contains("topsecret"),
        "owner sees the value"
    );
}

#[tokio::test]
async fn webhooks_require_a_valid_signature() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let toml =
        "[agent]\nid = \"invoice-bot\"\nname = \"Invoice bot\"\n\n[trigger]\non = \"invoice\"\n";
    let (s, v) = raw_post(
        &app,
        "/api/agents/import",
        toml.as_bytes(),
        &[
            ("authorization", &format!("Bearer {admin}")),
            ("content-type", "text/plain"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let agent = v["id"].as_str().unwrap().to_string();
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    let secret = v["webhook_secret"].as_str().unwrap().to_string();

    let body = br#"{"invoice": 42}"#;
    let (s, _) = raw_post(&app, "/api/webhooks/invoice", body, &[]).await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "unsigned");
    let bad = sign("wrong", body);
    let (s, _) = raw_post(
        &app,
        "/api/webhooks/invoice",
        body,
        &[("x-takoia-signature", &bad)],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "wrong secret");
    let good = sign(&secret, body);
    let (s, _) = raw_post(
        &app,
        "/api/webhooks/invoice",
        b"tampered",
        &[("x-takoia-signature", &good)],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "tampered body");
    let (s, v) = raw_post(
        &app,
        "/api/webhooks/invoice",
        body,
        &[("x-hub-signature-256", &good)],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    assert_eq!(v["triggered"].as_array().unwrap().len(), 1);
    assert_eq!(v["rejected"], 0);

    let (_, v) = call(&app, Method::GET, "/api/jobs", Some(&admin), None, &[]).await;
    let jobs = v["jobs"].as_array().unwrap();
    assert_eq!(jobs.len(), 1);
    assert_eq!(jobs[0]["status"], "queued");

    // Rotation invalidates the old secret at once.
    let (s, v) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/webhook-secret/rotate"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_ne!(v["webhook_secret"].as_str().unwrap(), secret);
    let (s, _) = raw_post(
        &app,
        "/api/webhooks/invoice",
        body,
        &[("x-takoia-signature", &good)],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED, "old secret after rotation");
}

#[tokio::test]
async fn memory_purge_is_admin_only_and_scoped_to_agent_topics() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (_, bob) = member(&app, &admin, "bob@example.test").await;
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/memory/purge?topic=takoia/agent/x",
        Some(&bob),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/memory/purge?topic=something/else",
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/memory/purge?topic=takoia/agent/does-not-exist",
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
}

/// A second tenant with one agent, created straight in the database (there is
/// no sign-up for extra accounts yet).
async fn foreign_agent(app: &TestApp) -> String {
    sqlx::query("INSERT INTO accounts (id, name) VALUES ('acct-b', 'Other')")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query(
        "INSERT INTO agents (id, account_id, name, webhook_secret) VALUES ('agent-b', 'acct-b', 'Theirs', 'sekrit')",
    )
    .execute(&app.db)
    .await
    .unwrap();
    "agent-b".to_string()
}

#[tokio::test]
async fn admins_are_owners_only_inside_their_own_account() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let other = foreign_agent(&app).await;
    let (s, _) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{other}"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "another tenant's agent");
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{other}/webhook-secret/rotate"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/objectives",
        Some(&admin),
        Some(json!({ "agent_id": other, "title": "t", "prompt": "run" })),
        &[],
    )
    .await;
    assert_eq!(
        s,
        StatusCode::FORBIDDEN,
        "cannot run another tenant's agent"
    );
    // Lists never leak the other tenant.
    let (_, v) = call(&app, Method::GET, "/api/agents", Some(&admin), None, &[]).await;
    assert!(v["agents"]
        .as_array()
        .unwrap()
        .iter()
        .all(|a| a["id"] != "agent-b"));
    let (_, v) = call(&app, Method::GET, "/api/schedules", Some(&admin), None, &[]).await;
    assert_eq!(v["schedules"].as_array().unwrap().len(), 0);
}

#[tokio::test]
async fn import_cannot_take_over_an_existing_agent() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (_, bob) = member(&app, &admin, "bob@example.test").await;
    let other = foreign_agent(&app).await;
    let toml = format!("[agent]\nid = \"{other}\"\nname = \"Hijacked\"\n");
    for tok in [&admin, &bob] {
        let (s, _) = raw_post(
            &app,
            "/api/agents/import",
            toml.as_bytes(),
            &[
                ("authorization", &format!("Bearer {tok}")),
                ("content-type", "text/plain"),
            ],
        )
        .await;
        assert_eq!(
            s,
            StatusCode::FORBIDDEN,
            "re-importing someone else's agent id"
        );
    }
    let (name,): (String,) = sqlx::query_as("SELECT name FROM agents WHERE id = 'agent-b'")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(name, "Theirs", "the victim agent is untouched");
    // A member re-importing an agent they own is a legitimate edit.
    let mine = "[agent]\nid = \"bobs-agent\"\nname = \"v1\"\n";
    let (s, _) = raw_post(
        &app,
        "/api/agents/import",
        mine.as_bytes(),
        &[
            ("authorization", &format!("Bearer {bob}")),
            ("content-type", "text/plain"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let mine2 = "[agent]\nid = \"bobs-agent\"\nname = \"v2\"\n";
    let (s, _) = raw_post(
        &app,
        "/api/agents/import",
        mine2.as_bytes(),
        &[
            ("authorization", &format!("Bearer {bob}")),
            ("content-type", "text/plain"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
}

#[tokio::test]
async fn editor_round_trip_keeps_masked_secrets_intact() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (bob_id, bob) = member(&app, &admin, "bob@example.test").await;
    let (_, v) = call(
        &app,
        Method::POST,
        "/api/agents",
        Some(&admin),
        Some(json!({ "name": "Alerter" })),
        &[],
    )
    .await;
    let agent = v["id"].as_str().unwrap().to_string();
    let secret_url = "https://discord.com/api/webhooks/1/topsecret";
    call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&admin),
        Some(json!({ "steps": [{ "step_type": "action", "options": { "tool_params": { "discord_webhook": secret_url, "a2a_calls": [{ "url": "https://peer", "key": "k1" }] } } }] })),
        &[],
    )
    .await;
    call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/permissions"),
        Some(&admin),
        Some(json!({ "user_id": bob_id, "role": "editor" })),
        &[],
    )
    .await;

    // Bob reads (masked), edits the prompt, saves what he saw.
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}"),
        Some(&bob),
        None,
        &[],
    )
    .await;
    let action = v["steps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["step_type"] == "action")
        .unwrap()
        .clone();
    let masked: Value = serde_json::from_str(action["options"].as_str().unwrap()).unwrap();
    assert_eq!(masked["tool_params"]["discord_webhook"], "***");
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&bob),
        Some(json!({ "steps": [{ "step_type": "action", "system_prompt": "be brief", "options": masked }] })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);

    let (stored,): (String,) = sqlx::query_as(
        "SELECT options FROM agent_step_configs WHERE agent_id = ? AND step_type = 'action'",
    )
    .bind(&agent)
    .fetch_one(&app.db)
    .await
    .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        stored["tool_params"]["discord_webhook"], secret_url,
        "real secret survives the masked write"
    );
    assert_eq!(stored["tool_params"]["a2a_calls"][0]["key"], "k1");
}

#[tokio::test]
async fn video_analysis_without_an_agent_is_admin_only() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (_, bob) = member(&app, &admin, "bob@example.test").await;
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/video/analyze",
        Some(&bob),
        Some(json!({ "frames": ["aGVsbG8="] })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN);
}

#[tokio::test]
async fn editor_cannot_redirect_credentials_to_another_host() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (bob_id, bob) = member(&app, &admin, "bob@example.test").await;
    let (_, v) = call(
        &app,
        Method::POST,
        "/api/agents",
        Some(&admin),
        Some(json!({ "name": "Caller" })),
        &[],
    )
    .await;
    let agent = v["id"].as_str().unwrap().to_string();
    call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&admin),
        Some(json!({ "steps": [{ "step_type": "action", "options": { "tool_params": { "a2a_calls": [{ "url": "https://peer/invoke", "key": "real-key" }] } } }] })),
        &[],
    )
    .await;
    call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/permissions"),
        Some(&admin),
        Some(json!({ "user_id": bob_id, "role": "editor" })),
        &[],
    )
    .await;

    // Bob keeps the mask but points the call at his own host, and tries to add
    // a connector reference and a webhook of his own.
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&bob),
        Some(
            json!({ "steps": [{ "step_type": "action", "options": { "tool_params": {
            "a2a_calls": [{ "url": "https://attacker.example/x", "key": "***" }],
            "discord_connector": "alerts",
            "discord_webhook": "https://attacker.example/hook",
            "symbol": "MSFT"
        } } }] }),
        ),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (stored,): (String,) = sqlx::query_as(
        "SELECT options FROM agent_step_configs WHERE agent_id = ? AND step_type = 'action'",
    )
    .bind(&agent)
    .fetch_one(&app.db)
    .await
    .unwrap();
    let stored: Value = serde_json::from_str(&stored).unwrap();
    assert_eq!(
        stored["tool_params"]["a2a_calls"][0]["url"], "https://peer/invoke",
        "URL cannot be moved by an editor"
    );
    assert_eq!(stored["tool_params"]["a2a_calls"][0]["key"], "real-key");
    assert!(stored["tool_params"].get("discord_connector").is_none());
    assert!(stored["tool_params"].get("discord_webhook").is_none());
    assert_eq!(
        stored["tool_params"]["symbol"], "MSFT",
        "non-credential params are editable"
    );

    // The owner can move it.
    let (s, _) = call(
        &app,
        Method::PUT,
        &format!("/api/agents/{agent}/steps"),
        Some(&admin),
        Some(json!({ "steps": [{ "step_type": "action", "options": { "tool_params": { "a2a_calls": [{ "url": "https://peer2/invoke", "key": "real-key" }] } } }] })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (stored,): (String,) = sqlx::query_as(
        "SELECT options FROM agent_step_configs WHERE agent_id = ? AND step_type = 'action'",
    )
    .bind(&agent)
    .fetch_one(&app.db)
    .await
    .unwrap();
    assert!(stored.contains("peer2"));
}

#[tokio::test]
async fn malformed_import_is_a_client_error() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (s, v) = raw_post(
        &app,
        "/api/agents/import",
        b"this is not toml = [",
        &[
            ("authorization", &format!("Bearer {admin}")),
            ("content-type", "text/plain"),
        ],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    assert!(v["error"]
        .as_str()
        .unwrap()
        .contains("invalid agent definition"));
}

/// Publish an agent and mint a consumer key for `token`'s account.
async fn published_agent_and_key(app: &TestApp, admin: &str) -> (String, String) {
    let (_, v) = call(
        app,
        Method::POST,
        "/api/agents",
        Some(admin),
        Some(json!({ "name": "Expert", "autonomy_level": "full_auto" })),
        &[],
    )
    .await;
    let agent = v["id"].as_str().unwrap().to_string();
    let (s, _) = call(
        app,
        Method::POST,
        &format!("/api/agents/{agent}/publish"),
        Some(admin),
        Some(json!({ "visibility": "public", "price_per_1k_output_tokens": 1.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    (agent, String::new())
}

/// A consumer key belonging to a second account, created straight in the DB.
async fn foreign_consumer_key(app: &TestApp) -> String {
    sqlx::query("INSERT INTO accounts (id, name) VALUES ('acct-c', 'Consumer Co') ON CONFLICT(id) DO NOTHING")
        .execute(&app.db)
        .await
        .unwrap();
    let key = "sk_takoia_consumer_test_key";
    let hash = {
        use sha2::Digest;
        format!("{:x}", sha2::Sha256::digest(key.as_bytes()))
    };
    sqlx::query("INSERT INTO api_keys (id, account_id, name, key_hash, key_prefix) VALUES ('k-c', 'acct-c', 'c', ?, 'sk_takoia_consu')")
        .bind(hash)
        .execute(&app.db)
        .await
        .unwrap();
    key.to_string()
}

#[tokio::test]
async fn consumer_memory_is_a_fork_the_consumer_can_read_and_erase() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (agent, _) = published_agent_and_key(&app, &admin).await;
    let key = foreign_consumer_key(&app).await;

    // Owner memory and consumer fork live side by side in the mirror.
    let owner = crate::memory::MemoryScope::owner(&agent);
    let fork = crate::memory::MemoryScope::consumer(&agent, "acct-c");
    let memory = crate::memory::Memory::new(
        app.db.clone(),
        app._dir.join("icm.db").to_string_lossy().into_owned(),
    );
    memory
        .store(&owner, "preference", "publisher curated fact")
        .await
        .unwrap();
    memory
        .store(&fork, "interaction", "consumer asked about pricing")
        .await
        .unwrap();
    assert_eq!(memory.list(&owner).await.unwrap().len(), 1);
    assert_eq!(memory.list(&fork).await.unwrap().len(), 1);

    // The consumer sees only their fork through the key-authed API.
    let (s, v) = call(
        &app,
        Method::GET,
        &format!("/api/v1/agents/{agent}/memory"),
        None,
        None,
        &[("authorization", &format!("Bearer {key}"))],
    )
    .await;
    assert_eq!(s, StatusCode::OK, "{v}");
    let mems = v["memories"].as_array().unwrap();
    assert_eq!(mems.len(), 1);
    assert_eq!(mems[0]["content"], "consumer asked about pricing");
    // No key: 401. Unpublished/unknown agent: 404.
    let (s, _) = call(
        &app,
        Method::GET,
        &format!("/api/v1/agents/{agent}/memory"),
        None,
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::UNAUTHORIZED);
    let (s, _) = call(
        &app,
        Method::GET,
        "/api/v1/agents/nope/memory",
        None,
        None,
        &[("authorization", &format!("Bearer {key}"))],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);

    // Erasing the fork leaves the owner's memory alone.
    let (s, _) = call(
        &app,
        Method::DELETE,
        &format!("/api/v1/agents/{agent}/memory"),
        None,
        None,
        &[("authorization", &format!("Bearer {key}"))],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(memory.list(&fork).await.unwrap().len(), 0);
    assert_eq!(
        memory.list(&owner).await.unwrap().len(),
        1,
        "owner memory untouched"
    );

    // And the owner's UI list never shows a consumer's rows.
    memory.store(&fork, "interaction", "again").await.unwrap();
    let (_, v) = call(
        &app,
        Method::GET,
        &format!("/api/agents/{agent}/memories"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(v["memories"].as_array().unwrap().len(), 1);

    // Admin purge of the owner topic keeps the fork; purge of the fork topic works too.
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/memory/purge?topic=takoia/agent/{agent}"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(memory.list(&owner).await.unwrap().len(), 0);
    assert_eq!(memory.list(&fork).await.unwrap().len(), 1);
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/memory/purge?topic=takoia/agent/{agent}/consumer/acct-c"),
        Some(&admin),
        None,
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(memory.list(&fork).await.unwrap().len(), 0);
}

#[tokio::test]
async fn invoke_requires_credit_and_respects_the_key_rate_limit() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (agent, _) = published_agent_and_key(&app, &admin).await;
    let key = foreign_consumer_key(&app).await;
    let auth = [("authorization", &*format!("Bearer {key}"))];

    // No credit: the call is refused before any token is spent, with a 402.
    let (s, v) = call(
        &app,
        Method::POST,
        &format!("/api/v1/agents/{agent}/invoke"),
        None,
        Some(json!({ "input": "hello" })),
        &auth,
    )
    .await;
    assert_eq!(s, StatusCode::PAYMENT_REQUIRED, "{v}");
    assert!(v["error"].as_str().unwrap().contains("insufficient credit"));
    let (n,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM jobs")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(n, 0, "no job was created");

    // Admin tops the consumer account up; the consumer sees it.
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/accounts/acct-c/credit",
        Some(&admin),
        Some(json!({ "delta_usd": 10.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/accounts/nope/credit",
        Some(&admin),
        Some(json!({ "delta_usd": 1.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::NOT_FOUND);
    let (_, bob) = member(&app, &admin, "bob@example.test").await;
    let (s, _) = call(
        &app,
        Method::POST,
        "/api/accounts/acct-c/credit",
        Some(&bob),
        Some(json!({ "delta_usd": 1.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::FORBIDDEN, "top-up is admin-only");

    // With credit the run is admitted. There is no LLM in the test process, so
    // the run fails — and the hold must be released, not leaked.
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/v1/agents/{agent}/invoke"),
        None,
        Some(json!({ "input": "hello" })),
        &auth,
    )
    .await;
    assert_eq!(s, StatusCode::INTERNAL_SERVER_ERROR);
    let (holds,): (i64,) = sqlx::query_as("SELECT COUNT(*) FROM credit_hold")
        .fetch_one(&app.db)
        .await
        .unwrap();
    assert_eq!(holds, 0, "failed run released its hold");
    let bal = crate::billing::balance(&app.db, "acct-c").await.unwrap();
    assert!(
        (bal.balance_usd - 10.0).abs() < 1e-9,
        "nothing charged for a failed run"
    );

    // Rate limit: cap the key at 1/min and pre-load one settled call.
    sqlx::query("UPDATE api_keys SET rate_limit_per_min = 1 WHERE id = 'k-c'")
        .execute(&app.db)
        .await
        .unwrap();
    sqlx::query("INSERT INTO credit_ledger (id, account_id, api_key_id, delta_usd, reason) VALUES ('l1', 'acct-c', 'k-c', 0, 'invoke')").execute(&app.db).await.unwrap();
    let (s, v) = call(
        &app,
        Method::POST,
        &format!("/api/v1/agents/{agent}/invoke"),
        None,
        Some(json!({ "input": "hello" })),
        &auth,
    )
    .await;
    assert_eq!(s, StatusCode::TOO_MANY_REQUESTS, "{v}");
}

#[tokio::test]
async fn publish_enforces_the_price_floor_when_configured() {
    let app = app().await;
    let admin = setup_admin(&app).await;
    let (_, v) = call(
        &app,
        Method::POST,
        "/api/agents",
        Some(&admin),
        Some(json!({ "name": "Cheap" })),
        &[],
    )
    .await;
    let agent = v["id"].as_str().unwrap().to_string();
    // Floor is 0 in tests: free publishing is allowed, negative prices are not.
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/publish"),
        Some(&admin),
        Some(json!({ "visibility": "public", "price_per_1k_output_tokens": -1.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/publish"),
        Some(&admin),
        Some(json!({ "visibility": "public", "revenue_share": 1.5 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::BAD_REQUEST);
    let (s, _) = call(
        &app,
        Method::POST,
        &format!("/api/agents/{agent}/publish"),
        Some(&admin),
        Some(json!({ "visibility": "public", "price_per_1k_output_tokens": 0.0 })),
        &[],
    )
    .await;
    assert_eq!(s, StatusCode::OK);
    // The consumer's balance view works for any member.
    let (s, v) = call(&app, Method::GET, "/api/credit", Some(&admin), None, &[]).await;
    assert_eq!(s, StatusCode::OK);
    assert_eq!(v["balance"]["balance_usd"], 0.0);
}
