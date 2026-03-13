mod analyzer;
mod crawler;
mod db;
mod handlers;
mod models;
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

    let state = Arc::new(AppState { pool, tmpl, tx, llm_client });

    // Rotas
    let app = Router::new()
        .route("/", get(handlers::index))
        .route("/analyze", post(handlers::post_analyze))
        .route("/jobs/:job_id", get(handlers::job_progress))
        .route("/jobs/:job_id/results", get(handlers::job_dashboard))
        .route("/jobs/:job_id/status", get(handlers::job_status_sse))
        .route("/jobs/:job_id/pages/:page_id", get(handlers::page_detail))
        .route("/jobs/:job_id/export", get(handlers::export_results))
        .route("/jobs/:job_id/delete", post(handlers::delete_job))
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

    // Filtro personalizado: formatar score como percentual
    env.add_filter("pct", |val: f64| format!("{:.0}%", val * 100.0));
    env.add_filter("badge_color", |badge: &str| match badge {
        "excellent" => "green",
        "moderate" => "yellow",
        _ => "red",
    });

    env
}
