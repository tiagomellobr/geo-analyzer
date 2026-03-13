-- Jobs de análise
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    site_url TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    -- pending | crawling | analyzing | completed | failed
    total_pages INTEGER NOT NULL DEFAULT 0,
    processed_pages INTEGER NOT NULL DEFAULT 0,
    created_at TEXT NOT NULL,
    updated_at TEXT NOT NULL,
    error_message TEXT
);

-- Páginas analisadas
CREATE TABLE IF NOT EXISTS pages (
    id TEXT PRIMARY KEY,
    job_id TEXT NOT NULL REFERENCES jobs(id) ON DELETE CASCADE,
    url TEXT NOT NULL,
    title TEXT,
    word_count INTEGER NOT NULL DEFAULT 0,
    -- Scores GEO (0.0 – 1.0)
    score_cite_sources REAL NOT NULL DEFAULT 0,
    score_quotation_addition REAL NOT NULL DEFAULT 0,
    score_statistics_addition REAL NOT NULL DEFAULT 0,
    score_fluency REAL NOT NULL DEFAULT 0,
    score_authoritative_tone REAL NOT NULL DEFAULT 0,
    score_technical_terms REAL NOT NULL DEFAULT 0,
    score_easy_to_understand REAL NOT NULL DEFAULT 0,
    score_content_structure REAL NOT NULL DEFAULT 0,
    score_metadata_quality REAL NOT NULL DEFAULT 0,
    score_schema_markup REAL NOT NULL DEFAULT 0,
    score_content_depth REAL NOT NULL DEFAULT 0,
    geo_score REAL NOT NULL DEFAULT 0,
    -- Recomendações em JSON
    recommendations TEXT NOT NULL DEFAULT '[]',
    -- Metadados extras em JSON
    meta_description TEXT,
    has_og_tags INTEGER NOT NULL DEFAULT 0,
    has_schema_markup INTEGER NOT NULL DEFAULT 0,
    analyzed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pages_job_id ON pages(job_id);
CREATE INDEX IF NOT EXISTS idx_pages_geo_score ON pages(geo_score);
