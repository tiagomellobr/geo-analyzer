use std::sync::Arc;
use uuid::Uuid;

use crate::{
    analyzer,
    crawler,
    db,
    models::Page,
    AppState, StatusEvent,
};

pub async fn run_analysis(state: Arc<AppState>, job_id: String) {
    let pool = &state.pool;

    // Buscar job
    let job = match db::get_job(pool, &job_id).await {
        Ok(Some(j)) => j,
        _ => return,
    };

    // Status: crawling
    let _ = db::update_job_status(pool, &job_id, "crawling", 0, 0, None).await;
    send_event(&state, &job_id, "crawling", 0, 0, None);

    // Construir HTTP client
    let client = match crawler::build_client().await {
        Ok(c) => c,
        Err(e) => {
            let msg = format!("Erro ao criar cliente HTTP: {e}");
            let _ = db::update_job_status(pool, &job_id, "failed", 0, 0, Some(&msg)).await;
            send_event(&state, &job_id, "failed", 0, 0, Some(msg));
            return;
        }
    };

    // Descobrir URLs do sitemap
    let discovery = match crawler::discover_urls(&client, &job.site_url).await {
        Ok(d) => d,
        Err(e) => {
            let msg = format!("Erro ao descobrir URLs: {e}");
            let _ = db::update_job_status(pool, &job_id, "failed", 0, 0, Some(&msg)).await;
            send_event(&state, &job_id, "failed", 0, 0, Some(msg));
            return;
        }
    };

    let urls = discovery.urls;
    let sitemap_warning = discovery.warning;

    let total = urls.len() as i64;
    let _ = db::update_job_status(pool, &job_id, "analyzing", total, 0, None).await;
    send_event(&state, &job_id, "analyzing", total, 0, None);

    // Baixar páginas
    let pages = crawler::crawl_pages(&client, urls).await;
    let actual_total = pages.len() as i64;

    // Analisar cada página
    let mut processed = 0i64;
    for crawled in pages {
        // Verificar cache LLM antes de chamar inferência
        let cached_llm = db::get_llm_cache(pool, &crawled.url).await.ok().flatten();
        let had_cache = cached_llm.is_some();
        if had_cache {
            tracing::debug!("Cache LLM hit para {}", crawled.url);
        }

        let result = analyzer::analyze_page(&crawled, Some(&state.llm_client), cached_llm).await;

        // Persistir resultado fresco do LLM no cache
        if !had_cache {
            if let Some(ref analysis) = result.llm_analysis {
                if let Err(e) = db::set_llm_cache(pool, &crawled.url, analysis).await {
                    tracing::warn!("Falha ao salvar cache LLM para {}: {e}", crawled.url);
                }
            }
        }

        let scores = &result.scores;
        let geo_score = scores.global_score();

        // Gerar resumo LLM para a página
        let page_title = crawled.title.as_deref().unwrap_or("Sem título");
        let llm_summary = state
            .llm_client
            .generate_summary(page_title, scores)
            .await;
        if llm_summary.is_none() {
            tracing::debug!("Resumo LLM não gerado para {}", crawled.url);
        }

        let page = Page {
            id: Uuid::new_v4().to_string(),
            job_id: job_id.clone(),
            url: crawled.url.clone(),
            title: crawled.title,
            word_count: crawled.word_count as i64,
            score_cite_sources: scores.cite_sources,
            score_quotation_addition: scores.quotation_addition,
            score_statistics_addition: scores.statistics_addition,
            score_fluency: scores.fluency,
            score_authoritative_tone: scores.authoritative_tone,
            score_technical_terms: scores.technical_terms,
            score_easy_to_understand: scores.easy_to_understand,
            score_content_structure: scores.content_structure,
            score_metadata_quality: scores.metadata_quality,
            score_schema_markup: scores.schema_markup,
            score_content_depth: scores.content_depth,
            geo_score,
            recommendations: serde_json::to_string(&result.recommendations)
                .unwrap_or_else(|_| "[]".to_string()),
            meta_description: crawled.meta_description,
            has_og_tags: result.has_og_tags as i64,
            has_schema_markup: result.has_schema_markup as i64,
            analyzed_at: chrono::Utc::now().to_rfc3339(),
            llm_summary,
        };

        if let Err(e) = db::insert_page(pool, &page).await {
            tracing::warn!("Erro ao salvar página {}: {}", crawled.url, e);
        }

        processed += 1;
        let _ =
            db::update_job_status(pool, &job_id, "analyzing", actual_total, processed, None).await;
        send_event(&state, &job_id, "analyzing", actual_total, processed, None);
    }

    // Concluído (inclui aviso de sitemap, se houver)
    let warning_ref = sitemap_warning.as_deref();
    let _ = db::update_job_status(pool, &job_id, "completed", actual_total, processed, warning_ref).await;
    send_event(&state, &job_id, "completed", actual_total, processed, sitemap_warning);
}

fn send_event(
    state: &AppState,
    job_id: &str,
    status: &str,
    total: i64,
    processed: i64,
    error: Option<String>,
) {
    let evt = StatusEvent {
        job_id: job_id.to_string(),
        status: status.to_string(),
        total_pages: total,
        processed_pages: processed,
        error_message: error,
    };
    // Ignora erros (sem receptores)
    let _ = state.tx.send(evt);
}
