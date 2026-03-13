use anyhow::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::str::FromStr;

use crate::models::{Job, Page};

pub async fn create_pool(database_url: &str) -> Result<SqlitePool> {
    let opts = SqliteConnectOptions::from_str(database_url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect_with(opts)
        .await?;
    run_migrations(&pool).await?;
    Ok(pool)
}

async fn run_migrations(pool: &SqlitePool) -> Result<()> {
    sqlx::query(include_str!("../../migrations/001_initial.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Jobs ────────────────────────────────────────────────────────────────────

pub async fn insert_job(pool: &SqlitePool, job: &Job) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, site_url, status, total_pages, processed_pages, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&job.id)
    .bind(&job.site_url)
    .bind(&job.status)
    .bind(job.total_pages)
    .bind(job.processed_pages)
    .bind(&job.created_at)
    .bind(&job.updated_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn update_job_status(
    pool: &SqlitePool,
    id: &str,
    status: &str,
    total_pages: i64,
    processed_pages: i64,
    error: Option<&str>,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "UPDATE jobs SET status=?, total_pages=?, processed_pages=?, updated_at=?, error_message=? WHERE id=?",
    )
    .bind(status)
    .bind(total_pages)
    .bind(processed_pages)
    .bind(&now)
    .bind(error)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_job(pool: &SqlitePool, id: &str) -> Result<Option<Job>> {
    let job = sqlx::query_as::<_, Job>("SELECT * FROM jobs WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(job)
}

pub async fn list_jobs(pool: &SqlitePool) -> Result<Vec<Job>> {
    let jobs =
        sqlx::query_as::<_, Job>("SELECT * FROM jobs ORDER BY created_at DESC LIMIT 20")
            .fetch_all(pool)
            .await?;
    Ok(jobs)
}

// ─── Pages ───────────────────────────────────────────────────────────────────

pub async fn insert_page(pool: &SqlitePool, page: &Page) -> Result<()> {
    sqlx::query(
        "INSERT INTO pages (
            id, job_id, url, title, word_count,
            score_cite_sources, score_quotation_addition, score_statistics_addition,
            score_fluency, score_authoritative_tone, score_technical_terms,
            score_easy_to_understand, score_content_structure, score_metadata_quality,
            score_schema_markup, score_content_depth, geo_score,
            recommendations, meta_description, has_og_tags, has_schema_markup, analyzed_at
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&page.id)
    .bind(&page.job_id)
    .bind(&page.url)
    .bind(&page.title)
    .bind(page.word_count)
    .bind(page.score_cite_sources)
    .bind(page.score_quotation_addition)
    .bind(page.score_statistics_addition)
    .bind(page.score_fluency)
    .bind(page.score_authoritative_tone)
    .bind(page.score_technical_terms)
    .bind(page.score_easy_to_understand)
    .bind(page.score_content_structure)
    .bind(page.score_metadata_quality)
    .bind(page.score_schema_markup)
    .bind(page.score_content_depth)
    .bind(page.geo_score)
    .bind(&page.recommendations)
    .bind(&page.meta_description)
    .bind(page.has_og_tags)
    .bind(page.has_schema_markup)
    .bind(&page.analyzed_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_pages_for_job(pool: &SqlitePool, job_id: &str) -> Result<Vec<Page>> {
    let pages = sqlx::query_as::<_, Page>(
        "SELECT * FROM pages WHERE job_id = ? ORDER BY geo_score ASC",
    )
    .bind(job_id)
    .fetch_all(pool)
    .await?;
    Ok(pages)
}

pub async fn delete_job(pool: &SqlitePool, id: &str) -> Result<()> {
    sqlx::query("DELETE FROM jobs WHERE id = ?")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn get_page(pool: &SqlitePool, id: &str) -> Result<Option<Page>> {
    let page = sqlx::query_as::<_, Page>("SELECT * FROM pages WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(page)
}
