use chrono::Utc;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Dados da sessão autenticada (persistidos no SQLite via tower-sessions)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionData {
    pub user_id: String,
    pub email: String,
}

/// Usuário autenticado
#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct User {
    pub id: String,
    pub email: String,
    pub password_hash: String,
    pub created_at: String,
}

impl User {
    pub fn new(email: String, password_hash: String) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            email,
            password_hash,
            created_at: Utc::now().to_rfc3339(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Job {
    pub id: String,
    pub user_id: Option<String>,
    pub site_url: String,
    pub status: String,
    pub total_pages: i64,
    pub processed_pages: i64,
    pub created_at: String,
    pub updated_at: String,
    pub error_message: Option<String>,
}

impl Job {
    pub fn new(site_url: String) -> Self {
        let now = Utc::now().to_rfc3339();
        Self {
            id: Uuid::new_v4().to_string(),
            user_id: None,
            site_url,
            status: "pending".to_string(),
            total_pages: 0,
            processed_pages: 0,
            created_at: now.clone(),
            updated_at: now,
            error_message: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, sqlx::FromRow)]
pub struct Page {
    pub id: String,
    pub job_id: String,
    pub url: String,
    pub title: Option<String>,
    pub word_count: i64,
    pub score_cite_sources: f64,
    pub score_quotation_addition: f64,
    pub score_statistics_addition: f64,
    pub score_fluency: f64,
    pub score_authoritative_tone: f64,
    pub score_technical_terms: f64,
    pub score_easy_to_understand: f64,
    pub score_content_structure: f64,
    pub score_metadata_quality: f64,
    pub score_schema_markup: f64,
    pub score_content_depth: f64,
    pub geo_score: f64,
    pub recommendations: String,
    pub meta_description: Option<String>,
    pub has_og_tags: i64,
    pub has_schema_markup: i64,
    pub analyzed_at: String,
    pub llm_summary: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeoScores {
    pub cite_sources: f64,
    pub quotation_addition: f64,
    pub statistics_addition: f64,
    pub fluency: f64,
    pub authoritative_tone: f64,
    pub technical_terms: f64,
    pub easy_to_understand: f64,
    pub content_structure: f64,
    pub metadata_quality: f64,
    pub schema_markup: f64,
    pub content_depth: f64,
}

impl GeoScores {
    pub fn global_score(&self) -> f64 {
        self.cite_sources * 0.20
            + self.quotation_addition * 0.20
            + self.statistics_addition * 0.15
            + self.fluency * 0.15
            + self.authoritative_tone * 0.10
            + self.technical_terms * 0.08
            + self.easy_to_understand * 0.07
            + self.content_structure * 0.03
            + self.metadata_quality * 0.02
    }

    pub fn badge(&self) -> &'static str {
        let score = self.global_score() * 100.0;
        if score >= 80.0 {
            "excellent"
        } else if score >= 50.0 {
            "moderate"
        } else {
            "critical"
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Recommendation {
    pub criterion: String,
    pub message: String,
    pub impact: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PageSummary {
    pub id: String,
    pub url: String,
    pub title: Option<String>,
    pub geo_score: f64,
    pub geo_score_pct: f64,
    pub badge: String,
    pub word_count: i64,
}

impl From<&Page> for PageSummary {
    fn from(p: &Page) -> Self {
        let pct = p.geo_score * 100.0;
        let badge = if pct >= 80.0 {
            "excellent"
        } else if pct >= 50.0 {
            "moderate"
        } else {
            "critical"
        }
        .to_string();
        Self {
            id: p.id.clone(),
            url: p.url.clone(),
            title: p.title.clone(),
            geo_score: p.geo_score,
            geo_score_pct: pct,
            badge,
            word_count: p.word_count,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobDashboard {
    pub job: Job,
    pub avg_score: f64,
    pub avg_score_pct: f64,
    /// Total de páginas no job (para o card de métricas)
    pub total_pages: i64,
    /// Número de páginas críticas (score < 50) no total
    pub critical_count: i64,
    /// Páginas da página atual (após filtro + ordenação + paginação)
    pub pages: Vec<PageSummary>,
    /// 5 piores páginas globais (independe de filtro)
    pub worst_pages: Vec<PageSummary>,
    /// Número de páginas após aplicar o filtro
    pub filtered_count: i64,
    // Paginação
    pub current_page: i64,
    pub total_pg_count: i64,
    pub has_prev: bool,
    pub has_next: bool,
    // Estado dos controles atuais (para construir URLs)
    pub current_sort: String,
    pub current_filter: String,
}
