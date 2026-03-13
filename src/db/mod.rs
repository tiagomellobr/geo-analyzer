use anyhow::Result;
use sqlx::{
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
    SqlitePool,
};
use std::str::FromStr;

use crate::models::{Job, Page, User};

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
    sqlx::query(include_str!("../../migrations/002_llm_cache.sql"))
        .execute(pool)
        .await?;
    // Migration 003 adiciona coluna que pode já existir em bancos anteriores;
    // ignora o erro de coluna duplicada para ser idempotente.
    if let Err(e) = sqlx::query(include_str!("../../migrations/003_llm_summary.sql"))
        .execute(pool)
        .await
    {
        if !e.to_string().contains("duplicate column name") {
            return Err(e.into());
        }
    }
    // Migration 004: tabela users (idempotente via IF NOT EXISTS)
    sqlx::query(include_str!("../../migrations/004_users.sql"))
        .execute(pool)
        .await?;
    // Migration 005: coluna user_id em jobs + índice (pode já existir em bancos anteriores)
    if let Err(e) = sqlx::query(include_str!("../../migrations/005_jobs_user_fk.sql"))
        .execute(pool)
        .await
    {
        if !e.to_string().contains("duplicate column name") {
            return Err(e.into());
        }
    }
    // Migration 006: tabela password_reset_tokens (idempotente via IF NOT EXISTS)
    sqlx::query(include_str!("../../migrations/006_reset_tokens.sql"))
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Jobs ────────────────────────────────────────────────────────────────────

pub async fn insert_job(pool: &SqlitePool, job: &Job) -> Result<()> {
    sqlx::query(
        "INSERT INTO jobs (id, user_id, site_url, status, total_pages, processed_pages, created_at, updated_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?)",
    )
    .bind(&job.id)
    .bind(&job.user_id)
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
            recommendations, meta_description, has_og_tags, has_schema_markup, analyzed_at,
            llm_summary
        ) VALUES (?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)",
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
    .bind(&page.llm_summary)
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

// ─── LLM Cache ───────────────────────────────────────────────────────────────

pub async fn get_llm_cache(
    pool: &SqlitePool,
    url: &str,
) -> Result<Option<crate::analyzer::llm::LlmAnalysis>> {
    use sqlx::Row;
    let row = sqlx::query(
        "SELECT fluency, authoritative_tone, technical_terms, easy_to_understand,
                fluency_rec, auth_rec, tech_rec, easy_rec
         FROM llm_cache WHERE url = ?",
    )
    .bind(url)
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|r: sqlx::sqlite::SqliteRow| crate::analyzer::llm::LlmAnalysis {
        fluency: r.get("fluency"),
        authoritative_tone: r.get("authoritative_tone"),
        technical_terms: r.get("technical_terms"),
        easy_to_understand: r.get("easy_to_understand"),
        fluency_recommendation: r.get("fluency_rec"),
        authoritative_tone_recommendation: r.get("auth_rec"),
        technical_terms_recommendation: r.get("tech_rec"),
        easy_to_understand_recommendation: r.get("easy_rec"),
    }))
}

pub async fn set_llm_cache(
    pool: &SqlitePool,
    url: &str,
    analysis: &crate::analyzer::llm::LlmAnalysis,
) -> Result<()> {
    let now = chrono::Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO llm_cache
             (url, fluency, authoritative_tone, technical_terms, easy_to_understand,
              fluency_rec, auth_rec, tech_rec, easy_rec, cached_at)
         VALUES (?, ?, ?, ?, ?, ?, ?, ?, ?, ?)
         ON CONFLICT(url) DO UPDATE SET
             fluency=excluded.fluency,
             authoritative_tone=excluded.authoritative_tone,
             technical_terms=excluded.technical_terms,
             easy_to_understand=excluded.easy_to_understand,
             fluency_rec=excluded.fluency_rec,
             auth_rec=excluded.auth_rec,
             tech_rec=excluded.tech_rec,
             easy_rec=excluded.easy_rec,
             cached_at=excluded.cached_at",
    )
    .bind(url)
    .bind(analysis.fluency)
    .bind(analysis.authoritative_tone)
    .bind(analysis.technical_terms)
    .bind(analysis.easy_to_understand)
    .bind(&analysis.fluency_recommendation)
    .bind(&analysis.authoritative_tone_recommendation)
    .bind(&analysis.technical_terms_recommendation)
    .bind(&analysis.easy_to_understand_recommendation)
    .bind(&now)
    .execute(pool)
    .await?;
    Ok(())
}

// ─── Users ───────────────────────────────────────────────────────────────────

pub async fn insert_user(pool: &SqlitePool, user: &User) -> Result<()> {
    sqlx::query(
        "INSERT INTO users (id, email, password_hash, created_at) VALUES (?, ?, ?, ?)",
    )
    .bind(&user.id)
    .bind(&user.email)
    .bind(&user.password_hash)
    .bind(&user.created_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_user_by_email(pool: &SqlitePool, email: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE email = ?")
        .bind(email)
        .fetch_optional(pool)
        .await?;
    Ok(user)
}

pub async fn get_user_by_id(pool: &SqlitePool, id: &str) -> Result<Option<User>> {
    let user = sqlx::query_as::<_, User>("SELECT * FROM users WHERE id = ?")
        .bind(id)
        .fetch_optional(pool)
        .await?;
    Ok(user)
}

pub async fn list_jobs_for_user(pool: &SqlitePool, user_id: &str) -> Result<Vec<Job>> {
    let jobs = sqlx::query_as::<_, Job>(
        "SELECT * FROM jobs WHERE user_id = ? ORDER BY created_at DESC LIMIT 20",
    )
    .bind(user_id)
    .fetch_all(pool)
    .await?;
    Ok(jobs)
}

pub async fn update_user_password(pool: &SqlitePool, user_id: &str, password_hash: &str) -> Result<()> {
    sqlx::query("UPDATE users SET password_hash = ? WHERE id = ?")
        .bind(password_hash)
        .bind(user_id)
        .execute(pool)
        .await?;
    Ok(())
}

// ─── Password Reset Tokens ────────────────────────────────────────────────────

pub async fn create_reset_token(pool: &SqlitePool, user_id: &str, token: &str, expires_at: &str) -> Result<()> {
    // Invalida tokens anteriores não utilizados do mesmo usuário
    sqlx::query("UPDATE password_reset_tokens SET used = 1 WHERE user_id = ? AND used = 0")
        .bind(user_id)
        .execute(pool)
        .await?;
    sqlx::query(
        "INSERT INTO password_reset_tokens (token, user_id, expires_at, used) VALUES (?, ?, ?, 0)",
    )
    .bind(token)
    .bind(user_id)
    .bind(expires_at)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn get_reset_token(pool: &SqlitePool, token: &str) -> Result<Option<(String, String, bool)>> {
    // Retorna (user_id, expires_at, used)
    let row = sqlx::query(
        "SELECT user_id, expires_at, used FROM password_reset_tokens WHERE token = ?",
    )
    .bind(token)
    .fetch_optional(pool)
    .await?;
    Ok(row.map(|r: sqlx::sqlite::SqliteRow| {
        use sqlx::Row;
        let used: i64 = r.get("used");
        (r.get("user_id"), r.get("expires_at"), used != 0)
    }))
}

pub async fn mark_reset_token_used(pool: &SqlitePool, token: &str) -> Result<()> {
    sqlx::query("UPDATE password_reset_tokens SET used = 1 WHERE token = ?")
        .bind(token)
        .execute(pool)
        .await?;
    Ok(())
}
