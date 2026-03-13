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

use crate::{
    db,
    models::{JobDashboard, PageSummary},
    AppState,
};

// ─── Tela inicial ─────────────────────────────────────────────────────────────

pub async fn index(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    let jobs = db::list_jobs(&state.pool).await.unwrap_or_default();
    let html = state
        .tmpl
        .get_template("index.html")
        .and_then(|t| {
            t.render(minijinja::context! {
                jobs => jobs,
            })
        })
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html)
}

// ─── Submit de análise ────────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct AnalyzeForm {
    pub url: String,
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
    Form(form): Form<AnalyzeForm>,
) -> impl IntoResponse {
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

    let job = crate::models::Job::new(site_url);
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
    Path(job_id): Path<String>,
) -> impl IntoResponse {
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
    Path(job_id): Path<String>,
) -> impl IntoResponse {
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

    let html = state
        .tmpl
        .get_template("progress.html")
        .and_then(|t| t.render(minijinja::context! { job => job }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── SSE: status do job ───────────────────────────────────────────────────────

pub async fn job_status_sse(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
) -> impl IntoResponse {
    let rx = state.tx.subscribe();
    let stream = BroadcastStream::new(rx)
        .filter_map(move |msg| {
            let jid = job_id.clone();
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
        .take(500); // limite de segurança

    Sse::new(stream).into_response()
}

// ─── Dashboard de resultados ──────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct DashboardQuery {
    pub sort: Option<String>,
    pub filter: Option<String>,
}

pub async fn job_dashboard(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Query(q): Query<DashboardQuery>,
) -> impl IntoResponse {
    let job = match db::get_job(&state.pool, &job_id).await {
        Ok(Some(j)) => j,
        _ => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado".to_string())).into_response()
        }
    };

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

    let mut summaries: Vec<PageSummary> = pages.iter().map(PageSummary::from).collect();

    // Filtrar
    if let Some(ref f) = q.filter {
        match f.as_str() {
            "critical" => summaries.retain(|p| p.geo_score_pct < 50.0),
            "moderate" => summaries.retain(|p| p.geo_score_pct >= 50.0 && p.geo_score_pct < 80.0),
            "excellent" => summaries.retain(|p| p.geo_score_pct >= 80.0),
            _ => {}
        }
    }

    // Ordenar
    match q.sort.as_deref() {
        Some("score_asc") => summaries.sort_by(|a, b| a.geo_score.partial_cmp(&b.geo_score).unwrap()),
        Some("score_desc") | None => {
            summaries.sort_by(|a, b| b.geo_score.partial_cmp(&a.geo_score).unwrap())
        }
        _ => {}
    }

    let avg_score = if !pages.is_empty() {
        pages.iter().map(|p| p.geo_score).sum::<f64>() / pages.len() as f64
    } else {
        0.0
    };

    let mut worst = summaries.clone();
    worst.sort_by(|a, b| a.geo_score.partial_cmp(&b.geo_score).unwrap());
    worst.truncate(5);

    let dashboard = JobDashboard {
        job,
        avg_score,
        avg_score_pct: avg_score * 100.0,
        pages: summaries,
        worst_pages: worst,
    };

    let html = state
        .tmpl
        .get_template("dashboard.html")
        .and_then(|t| t.render(minijinja::context! { d => dashboard }))
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── Detalhe da página ────────────────────────────────────────────────────────

pub async fn page_detail(
    State(state): State<Arc<AppState>>,
    Path((job_id, page_id)): Path<(String, String)>,
) -> impl IntoResponse {
    let job = match db::get_job(&state.pool, &job_id).await {
        Ok(Some(j)) => j,
        _ => {
            return (StatusCode::NOT_FOUND, Html("Job não encontrado".to_string())).into_response()
        }
    };

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
            })
        })
        .unwrap_or_else(|e| format!("Template error: {e}"));
    Html(html).into_response()
}

// ─── Exportação CSV / JSON ────────────────────────────────────────────────────

#[derive(Deserialize)]
pub struct ExportQuery {
    pub format: Option<String>,
}

pub async fn export_results(
    State(state): State<Arc<AppState>>,
    Path(job_id): Path<String>,
    Query(q): Query<ExportQuery>,
) -> impl IntoResponse {
    let pages = match db::get_pages_for_job(&state.pool, &job_id).await {
        Ok(p) => p,
        Err(_) => {
            return (StatusCode::NOT_FOUND, "Job não encontrado".to_string()).into_response()
        }
    };

    match q.format.as_deref().unwrap_or("json") {
        "csv" => {
            // Escapa campo CSV: envolve em aspas duplas e duplica aspas internas
            fn csv_field(s: &str) -> String {
                format!("\"{}\"", s.replace('"', "\"\""))
            }

            let mut csv = "url,title,geo_score,word_count,cite_sources,quotation,statistics,fluency,authoritative,technical,easy_to_understand,content_structure,metadata\n".to_string();
            for p in &pages {
                csv.push_str(&format!(
                    "{},{},{:.1},{},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2},{:.2}\n",
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
