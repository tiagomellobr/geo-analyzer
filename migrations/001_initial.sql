-- Jobs de análise
CREATE TABLE IF NOT EXISTS jobs (
    id TEXT PRIMARY KEY,
    site_url TEXT NOT NULL,
    status TEXT NOT NULL DEFAULT 'pending',
    -- pending | crawling | analyzing | completed | failed
    total_pages BIGINT NOT NULL DEFAULT 0,
    processed_pages BIGINT NOT NULL DEFAULT 0,
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
    word_count BIGINT NOT NULL DEFAULT 0,
    -- Scores GEO (0.0 – 1.0)
    score_cite_sources DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_quotation_addition DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_statistics_addition DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_fluency DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_authoritative_tone DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_technical_terms DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_easy_to_understand DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_content_structure DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_metadata_quality DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_schema_markup DOUBLE PRECISION NOT NULL DEFAULT 0,
    score_content_depth DOUBLE PRECISION NOT NULL DEFAULT 0,
    geo_score DOUBLE PRECISION NOT NULL DEFAULT 0,
    -- Recomendações em JSON
    recommendations TEXT NOT NULL DEFAULT '[]',
    -- Metadados extras em JSON
    meta_description TEXT,
    has_og_tags BIGINT NOT NULL DEFAULT 0,
    has_schema_markup BIGINT NOT NULL DEFAULT 0,
    analyzed_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS idx_pages_job_id ON pages(job_id);
CREATE INDEX IF NOT EXISTS idx_pages_geo_score ON pages(geo_score);
