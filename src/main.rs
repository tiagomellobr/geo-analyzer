mod analyzer;
mod crawler;
mod db;
mod handlers;
mod models;
mod pdf;
mod worker;

use std::sync::Arc;

use axum::{
    routing::{get, post},
    Router,
};
use minijinja::Environment;
use serde::{Deserialize, Serialize};
use sqlx::SqlitePool;
use tokio::sync::broadcast;
use tower_http::trace::TraceLayer;
use tower_sessions::{
    cookie::SameSite, Expiry, SessionManagerLayer,
};
use tower_sessions_sqlx_store::SqliteStore;
use tracing_subscriber::{layer::SubscriberExt, util::SubscriberInitExt};

/// Evento de progresso enviado por SSE
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatusEvent {
    pub job_id: String,
    pub status: String,
    pub total_pages: i64,
    pub processed_pages: i64,
    pub error_message: Option<String>,
}

/// Estado compartilhado da aplicação
pub struct AppState {
    pub pool: SqlitePool,
    pub tmpl: Environment<'static>,
    pub tx: broadcast::Sender<StatusEvent>,
    pub llm_client: analyzer::llm::LlmClient,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // Logging
    tracing_subscriber::registry()
        .with(tracing_subscriber::EnvFilter::new(
            std::env::var("RUST_LOG").unwrap_or_else(|_| "geo_analyzer=info,tower_http=info".into()),
        ))
        .with(tracing_subscriber::fmt::layer())
        .init();

    // Banco de dados
    let database_url =
        std::env::var("DATABASE_URL").unwrap_or_else(|_| "sqlite://geo_analyzer.db".to_string());
    let pool = db::create_pool(&database_url).await?;

    // Templates
    let tmpl = build_templates();

    // Canal SSE
    let (tx, _rx) = broadcast::channel::<StatusEvent>(128);

    // Cliente LLM (Ollama)
    let llm_client = analyzer::llm::LlmClient::new();
    if llm_client.is_available().await {
        tracing::info!("Ollama disponível em {} | modelo: {}", llm_client.host, llm_client.model);
        if let Err(e) = llm_client.ensure_model_pulled().await {
            tracing::warn!("Não foi possível puxar o modelo: {e}. Análise LLM usará fallback heurístico.");
        }
    } else {
        tracing::warn!("Ollama não encontrado em {}. Análise LLM usará fallback heurístico.", llm_client.host);
    }

    let state = Arc::new(AppState {
        pool: pool.clone(),
        tmpl,
        tx,
        llm_client,
    });

    // Sessão persistente no SQLite (tower-sessions)
    let session_store = SqliteStore::new(pool);
    session_store.migrate().await?;

    // Limpar sessões expiradas a cada 1 h em background
    let cleanup_store = session_store.clone();
    tokio::spawn(async move {
        use tower_sessions::session_store::ExpiredDeletion;
        let _ = cleanup_store
            .continuously_delete_expired(tokio::time::Duration::from_secs(3600))
            .await;
    });

    let secure_cookie = std::env::var("SESSION_SECURE")
        .map(|v| v == "true")
        .unwrap_or(false);

    let session_layer = SessionManagerLayer::new(session_store)
        .with_secure(secure_cookie)
        .with_same_site(SameSite::Strict)
        .with_http_only(true)
        .with_expiry(Expiry::OnInactivity(time::Duration::days(7)));

    // Rotas
    let app = Router::new()
        .route("/", get(handlers::index))
        .route("/register", get(handlers::auth::register_page))
        .route("/register", post(handlers::auth::register))
        .route("/login", get(handlers::auth::login_page))
        .route("/login", post(handlers::auth::login))
        .route("/logout", post(handlers::auth::logout))
        .route("/forgot-password", get(handlers::password_reset::forgot_password_page))
        .route("/forgot-password", post(handlers::password_reset::forgot_password))
        .route("/reset-password/:token", get(handlers::password_reset::reset_password_page))
        .route("/reset-password/:token", post(handlers::password_reset::reset_password))
        .route("/analyze", post(handlers::post_analyze))
        .route("/jobs/:job_id", get(handlers::job_progress))
        .route("/jobs/:job_id/results", get(handlers::job_dashboard))
        .route("/jobs/:job_id/status", get(handlers::job_status_sse))
        .route("/jobs/:job_id/pages/:page_id", get(handlers::page_detail))
        .route("/jobs/:job_id/export", get(handlers::export_results))
        .route("/jobs/:job_id/delete", post(handlers::delete_job))
        .layer(session_layer)
        .layer(TraceLayer::new_for_http())
        .with_state(state);

    let addr = std::env::var("BIND").unwrap_or_else(|_| "0.0.0.0:3000".to_string());
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    tracing::info!("GEO Analyzer rodando em http://{}", addr);
    axum::serve(listener, app).await?;

    Ok(())
}

fn build_templates() -> Environment<'static> {
    let mut env = Environment::new();

    env.add_template_owned(
        "base.html",
        include_str!("../templates/base.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "index.html",
        include_str!("../templates/index.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "progress.html",
        include_str!("../templates/progress.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "dashboard.html",
        include_str!("../templates/dashboard.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "page_detail.html",
        include_str!("../templates/page_detail.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "login.html",
        include_str!("../templates/login.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "register.html",
        include_str!("../templates/register.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "forgot_password.html",
        include_str!("../templates/forgot_password.html").to_string(),
    )
    .unwrap();
    env.add_template_owned(
        "reset_password.html",
        include_str!("../templates/reset_password.html").to_string(),
    )
    .unwrap();

    // Filtro personalizado: formatar score como percentual
    env.add_filter("pct", |val: f64| format!("{:.0}%", val * 100.0));
    env.add_filter("badge_color", |badge: &str| match badge {
        "excellent" => "green",
        "moderate" => "yellow",
        _ => "red",
    });

    env
}
