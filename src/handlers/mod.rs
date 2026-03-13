use axum::{
    extract::{Path, Query, State},
    http::{header, HeaderMap, StatusCode},
    response::{Html, IntoResponse, Sse},
    Form,
};
use serde::Deserialize;
use std::sync::Arc;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt as _;
use tower_sessions::Session;

use crate::{
    db,
    models::{Job, JobDashboard, Page, PageSummary},
    AppState,
};

pub mod auth;
pub mod csrf;

// ─── Tela inicial ─────────────────────────────────────────────────────────────

pub async fn index(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
) -> impl IntoResponse {
    let current_user = auth.0;
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let jobs = db::list_jobs_for_user(&state.pool, &current_user.user_id)
        .await
        .unwrap_or_default();
    let html = state
        .tmpl
        .get_template("index.html")
        .and_then(|t| {
            t.render(minijinja::context! {
                jobs => jobs,
                current_user => current_user,
                csrf_token => csrf_token,
            })
        })
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html)
}

// ─── Submit de análise ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AnalyzeForm {
    pub url: String,
    pub csrf_token: Option<String>,
}

#[derive(Deserialize)]
pub struct DeleteForm {
    pub csrf_token: Option<String>,
}

/// Verifica se a URL aponta para um host público (prevenção de SSRF).
fn is_safe_url(parsed: &url::Url) -> bool {
    match parsed.host() {
        Some(url::Host::Ipv4(ip)) => {
            !ip.is_private() && !ip.is_loopback() && !ip.is_link_local() && !ip.is_unspecified()
        }
        Some(url::Host::Ipv6(ip)) => !ip.is_loopback() && !ip.is_unspecified(),
        Some(url::Host::Domain(d)) => {
            let d = d.to_ascii_lowercase();
            d != "localhost"
                && !d.ends_with(".local")
                && !d.ends_with(".internal")
                && !d.ends_with(".localhost")
        }
        None => false,
    }
}

pub async fn post_analyze(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
    Form(form): Form<AnalyzeForm>,
) -> impl IntoResponse {
    let current_user = auth.0;
    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido. Recarregue a página e tente novamente.".to_string()),
        )
            .into_response();
    }
    // Sanitizar e validar URL
    let raw = form.url.trim().to_string();
    let site_url = if raw.starts_with("http://") || raw.starts_with("https://") {
        raw
    } else {
        format!("https://{}", raw)
    };

    let parsed = match url::Url::parse(&site_url) {
        Ok(u) => u,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Html("URL inválida. Por favor, insira uma URL válida.".to_string()),
            )
                .into_response();
        }
    };

    if !is_safe_url(&parsed) {
        return (
            StatusCode::BAD_REQUEST,
            Html("URL não permitida. Insira um domínio público válido.".to_string()),
        )
            .into_response();
    }

    let mut job = crate::models::Job::new(site_url);
    job.user_id = Some(current_user.user_id);
    if let Err(e) = db::insert_job(&state.pool, &job).await {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Html(format!("Erro ao criar job: {e}")),
        )
            .into_response();
    }

    // Disparar análise em background
    let state_clone = state.clone();
    let job_id = job.id.clone();
    tokio::spawn(async move {
        crate::worker::run_analysis(state_clone, job_id).await;
    });

    // Redirecionar para a tela de progresso (HTMX-friendly)
    let mut headers = HeaderMap::new();
    headers.insert(
        "HX-Redirect",
        format!("/jobs/{}", job.id).parse().unwrap(),
    );
    (StatusCode::OK, headers, Html(String::new())).into_response()
}

// ─── Excluir job ─────────────────────────────────────────────────────────────

pub async fn delete_job(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
    Path(job_id): Path<String>,
    Form(form): Form<DeleteForm>,
) -> impl IntoResponse {
    let current_user = auth.0;
    if !csrf::validate_csrf_token(&session, form.csrf_token.as_deref().unwrap_or("")).await {
        return (
            StatusCode::FORBIDDEN,
            Html("Token CSRF inválido.".to_string()),
        )
            .into_response();
    }
    match db::get_job(&state.pool, &job_id).await {
        Ok(Some(job)) if job.user_id.as_deref() == Some(&current_user.user_id) => {}
        Ok(Some(_)) => {
            return (StatusCode::FORBIDDEN, Html("Acesso negado.".to_string())).into_response();
        }
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado.".to_string())).into_response();
        }
        Err(e) => {
            tracing::error!("delete_job: DB error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Html("Erro interno.".to_string())).into_response();
        }
    }
    if let Err(e) = db::delete_job(&state.pool, &job_id).await {
        tracing::error!("delete_job: erro ao excluir {}: {}", job_id, e);
    }
    let mut headers = HeaderMap::new();
    headers.insert("HX-Redirect", "/".parse().unwrap());
    (StatusCode::OK, headers, Html(String::new())).into_response()
}

// ─── Tela de progresso ────────────────────────────────────────────────────────

pub async fn job_progress(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let current_user = auth.0;
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let job = match db::get_job(&state.pool, &job_id).await {
        Ok(Some(j)) => j,
        Ok(None) => {
            tracing::warn!("job_progress: job {} not found in DB", job_id);
            return (StatusCode::NOT_FOUND, Html("Job não encontrado".to_string())).into_response();
        }
        Err(e) => {
            tracing::error!("job_progress: DB error for job {}: {}", job_id, e);
            return (
                StatusCode::INTERNAL_SERVER_ERROR,
                Html(format!("Erro interno: {e}")),
            )
                .into_response();
        }
    };

    if job.user_id.as_deref() != Some(&current_user.user_id) {
        return (StatusCode::FORBIDDEN, Html("Acesso negado.".to_string())).into_response();
    }

    let html = state
        .tmpl
        .get_template("progress.html")
        .and_then(|t| t.render(minijinja::context! { job => job, current_user => current_user, csrf_token => csrf_token }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── SSE: status do job ───────────────────────────────────────────────────────

pub async fn job_status_sse(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let current_user = auth.0;
    // Verificar propriedade antes de expor o stream
    match db::get_job(&state.pool, &job_id).await {
        Ok(Some(job)) if job.user_id.as_deref() == Some(&current_user.user_id) => {}
        Ok(Some(_)) => {
            return (StatusCode::FORBIDDEN, Html("Acesso negado.".to_string())).into_response();
        }
        Ok(None) => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado.".to_string())).into_response();
        }
        Err(e) => {
            tracing::error!("job_status_sse: DB error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, Html("Erro interno.".to_string())).into_response();
        }
    }
    // Subscreve PRIMEIRO para não perder eventos enquanto lemos o DB.
    let rx = state.tx.subscribe();

    // Envia o estado atual como primeiro evento — resolve a race condition onde
    // o job termina antes do browser abrir a conexão SSE.
    let initial: Vec<Result<axum::response::sse::Event, std::convert::Infallible>> =
        match db::get_job(&state.pool, &job_id).await.ok().flatten() {
            Some(job) => {
                let evt = crate::StatusEvent {
                    job_id: job_id.clone(),
                    status: job.status,
                    total_pages: job.total_pages,
                    processed_pages: job.processed_pages,
                    error_message: job.error_message,
                };
                let data = serde_json::to_string(&evt).unwrap_or_default();
                vec![Ok(axum::response::sse::Event::default().data(data))]
            }
            None => vec![],
        };

    let job_id_clone = job_id.clone();
    let broadcast_stream = BroadcastStream::new(rx)
        .filter_map(move |msg| {
            let jid = job_id_clone.clone();
            match msg {
                Ok(evt) if evt.job_id == jid => {
                    let data = serde_json::to_string(&evt).unwrap_or_default();
                    Some(Ok::<_, std::convert::Infallible>(
                        axum::response::sse::Event::default().data(data),
                    ))
                }
                _ => None,
            }
        })
        .take(500);

    let stream = tokio_stream::iter(initial).chain(broadcast_stream);
    Sse::new(stream).into_response()
}

// ─── Dashboard de resultados ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub sort: Option<String>,
    pub filter: Option<String>,
    pub page: Option<i64>,
}

const PAGE_SIZE: i64 = 25;

pub async fn job_dashboard(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
    Path(job_id): Path<String>,
    Query(q): Query<DashboardQuery>,
) -> impl IntoResponse {
    let current_user = auth.0;
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let job = match db::get_job(&state.pool, &job_id).await {
        Ok(Some(j)) => j,
        _ => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado".to_string())).into_response()
        }
    };

    if job.user_id.as_deref() != Some(&current_user.user_id) {
        return (StatusCode::FORBIDDEN, Html("Acesso negado.".to_string())).into_response();
    }

    // Se ainda em andamento, redirecionar para progresso
    if job.status != "completed" && job.status != "failed" {
        let mut headers = HeaderMap::new();
        headers.insert(
            header::LOCATION,
            format!("/jobs/{}", job_id).parse().unwrap(),
        );
        return (StatusCode::SEE_OTHER, headers, Html(String::new())).into_response();
    }

    let pages = db::get_pages_for_job(&state.pool, &job_id)
        .await
        .unwrap_or_default();

    let all_summaries: Vec<PageSummary> = pages.iter().map(PageSummary::from).collect();

    // Estatísticas globais (independem de filtro/paginação)
    let total_pages_count = all_summaries.len() as i64;
    let critical_count = all_summaries.iter().filter(|p| p.badge == "critical").count() as i64;
    let avg_score = if !pages.is_empty() {
        pages.iter().map(|p| p.geo_score).sum::<f64>() / pages.len() as f64
    } else {
        0.0
    };

    // 5 piores páginas globais (antes de qualquer filtro)
    let mut worst = all_summaries.clone();
    worst.sort_by(|a, b| a.geo_score.partial_cmp(&b.geo_score).unwrap());
    worst.truncate(5);

    // Filtrar
    let mut filtered = all_summaries.clone();
    if let Some(ref f) = q.filter {
        match f.as_str() {
            "critical" => filtered.retain(|p| p.geo_score_pct < 50.0),
            "moderate" => filtered.retain(|p| p.geo_score_pct >= 50.0 && p.geo_score_pct < 80.0),
            "excellent" => filtered.retain(|p| p.geo_score_pct >= 80.0),
            _ => {}
        }
    }

    // Ordenar
    match q.sort.as_deref() {
        Some("score_asc") => {
            filtered.sort_by(|a, b| a.geo_score.partial_cmp(&b.geo_score).unwrap())
        }
        _ => filtered.sort_by(|a, b| b.geo_score.partial_cmp(&a.geo_score).unwrap()),
    }

    // Paginação
    let filtered_count = filtered.len() as i64;
    let total_pg_count = ((filtered_count + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let page_num = q.page.unwrap_or(1).max(1).min(total_pg_count);
    let offset = ((page_num - 1) * PAGE_SIZE) as usize;
    let paged: Vec<PageSummary> = filtered
        .into_iter()
        .skip(offset)
        .take(PAGE_SIZE as usize)
        .collect();

    let current_sort = q.sort.clone().unwrap_or_default();
    let current_filter = q.filter.clone().unwrap_or_default();

    let dashboard = JobDashboard {
        job,
        avg_score,
        avg_score_pct: avg_score * 100.0,
        total_pages: total_pages_count,
        critical_count,
        pages: paged,
        worst_pages: worst,
        filtered_count,
        current_page: page_num,
        total_pg_count,
        has_prev: page_num > 1,
        has_next: page_num < total_pg_count,
        current_sort,
        current_filter,
    };

    let html = state
        .tmpl
        .get_template("dashboard.html")
        .and_then(|t| t.render(minijinja::context! { d => dashboard, current_user => current_user, csrf_token => csrf_token }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── Detalhe da página ────────────────────────────────────────────────────────

pub async fn page_detail(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    session: Session,
    Path((job_id, page_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let current_user = auth.0;
    let csrf_token = csrf::get_or_create_csrf_token(&session).await;
    let job = match db::get_job(&state.pool, &job_id).await {
        Ok(Some(j)) => j,
        _ => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado".to_string())).into_response()
        }
    };

    if job.user_id.as_deref() != Some(&current_user.user_id) {
        return (StatusCode::FORBIDDEN, Html("Acesso negado.".to_string())).into_response();
    }

    let page = match db::get_page(&state.pool, &page_id).await {
        Ok(Some(p)) if p.job_id == job_id => p,
        _ => {
            return (StatusCode::NOT_FOUND, Html("Página não encontrada".to_string()))
                .into_response()
        }
    };

    let recs: Vec<crate::models::Recommendation> =
        serde_json::from_str(&page.recommendations).unwrap_or_default();

    let html = state
        .tmpl
        .get_template("page_detail.html")
        .and_then(|t| {
            t.render(minijinja::context! {
                job => job,
                page => page,
                recommendations => recs,
                geo_score_pct => page.geo_score * 100.0,
                current_user => current_user,
                csrf_token => csrf_token,
            })
        })
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── Exportação CSV / JSON ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
    /// Incluir seção detalhada por página no PDF (?detail=true)
    pub detail: Option<bool>,
}

pub async fn export_results(
    State(state): State<Arc<AppState>>,
    auth: auth::AuthUser,
    Path(job_id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    let current_user = auth.0;
    match db::get_job(&state.pool, &job_id).await {
        Ok(Some(job)) if job.user_id.as_deref() == Some(&current_user.user_id) => {}
        Ok(Some(_)) => {
            return (StatusCode::FORBIDDEN, "Acesso negado.".to_string()).into_response();
        }
        Ok(None) => {
            return (StatusCode::NOT_FOUND, "Job não encontrado.".to_string()).into_response();
        }
        Err(e) => {
            tracing::error!("export_results: DB error: {e}");
            return (StatusCode::INTERNAL_SERVER_ERROR, "Erro interno.".to_string()).into_response();
        }
    }
    let pages = match db::get_pages_for_job(&state.pool, &job_id).await {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::NOT_FOUND, "Job não encontrado".to_string()).into_response()
        }
    };

    match q.format.as_deref().unwrap_or("json") {
        "pdf" => return export_pdf(state, job_id, &pages, q.detail.unwrap_or(false)).await,
        "csv" => {
            // Escapa campo CSV: envolve em aspas duplas e duplica aspas internas
            fn csv_field(s: &str) -> String {
                format!("\"{}\"", s.replace('"', "\"\""))
            }

            let mut csv = "url,title,geo_score,word_count,cite_sources,quotation,statistics,fluency,authoritative,technical,easy_to_understand,content_structure,metadata,llm_summary\n".to_string();
            for p in &pages {
                csv.push_str(&format!(
                    "{},{},{:.1},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{}\n",
                    csv_field(&p.url),
                    csv_field(p.title.as_deref().unwrap_or("")),
                    p.geo_score * 100.0,
                    p.word_count,
                    p.score_cite_sources,
                    p.score_quotation_addition,
                    p.score_statistics_addition,
                    p.score_fluency,
                    p.score_authoritative_tone,
                    p.score_technical_terms,
                    p.score_easy_to_understand,
                    p.score_content_structure,
                    p.score_metadata_quality,
                    csv_field(p.llm_summary.as_deref().unwrap_or("")),
                ));
            }
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                "text/csv; charset=utf-8".parse().unwrap(),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"geo-{}.csv\"", job_id)
                    .parse()
                    .unwrap(),
            );
            (headers, csv).into_response()
        }
        _ => {
            let json = serde_json::to_string_pretty(&pages).unwrap_or_default();
            let mut headers = HeaderMap::new();
            headers.insert(
                header::CONTENT_TYPE,
                "application/json; charset=utf-8".parse().unwrap(),
            );
            headers.insert(
                header::CONTENT_DISPOSITION,
                format!("attachment; filename=\"geo-{}.json\"", job_id)
                    .parse()
                    .unwrap(),
            );
            (headers, json).into_response()
        }
    }
}

// ─── Exportação PDF ──────────────────────────────────────────────────────────

async fn export_pdf(
    state: std::sync::Arc<AppState>,
    job_id: String,
    pages: &[Page],
    include_detail: bool,
) -> axum::response::Response {
    let suffix = if include_detail { "detail" } else { "summary" };
    let cache_path = format!("/tmp/geo-pdf-{}-{}.pdf", job_id, suffix);

    // Verifica cache de 10 minutos
    let cached_bytes: Option<Vec<u8>> = match tokio::fs::metadata(&cache_path).await {
        Ok(meta) => {
            let fresh = meta
                .modified()
                .ok()
                .and_then(|m| m.elapsed().ok())
                .map(|e| e.as_secs() < 600)
                .unwrap_or(false);
            if fresh {
                tokio::fs::read(&cache_path).await.ok()
            } else {
                None
            }
        }
        Err(_) => None,
    };

    let pdf_bytes = if let Some(b) = cached_bytes {
        b
    } else {
        // Busca o job para os metadados da capa
        let job: Job = match db::get_job(&state.pool, &job_id).await {
            Ok(Some(j)) => j,
            _ => {
                return (StatusCode::NOT_FOUND, "Job não encontrado").into_response();
            }
        };

        // Geração é síncrona/CPU — executa em thread separada
        let pages_clone: Vec<Page> = pages.to_vec();
        let result = tokio::task::spawn_blocking(move || {
            crate::pdf::generate_pdf_bytes(&job, &pages_clone, include_detail)
        })
        .await;

        match result {
            Ok(Ok(bytes)) => {
                let _ = tokio::fs::write(&cache_path, &bytes).await;
                bytes
            }
            Ok(Err(e)) => {
                tracing::error!("Erro ao gerar PDF: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, format!("Erro ao gerar PDF: {e}"))
                    .into_response();
            }
            Err(e) => {
                tracing::error!("Panic na geração do PDF: {e}");
                return (StatusCode::INTERNAL_SERVER_ERROR, "Erro interno ao gerar PDF")
                    .into_response();
            }
        }
    };

    let filename = format!("geo-{}-{}.pdf", job_id, suffix);
    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        "application/pdf".parse().unwrap(),
    );
    headers.insert(
        header::CONTENT_DISPOSITION,
        format!("attachment; filename=\"{}\"", filename).parse().unwrap(),
    );
    (headers, pdf_bytes).into_response()
}
