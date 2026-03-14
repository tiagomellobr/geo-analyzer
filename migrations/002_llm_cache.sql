-- Cache de análise LLM por URL
-- Evita re-executar inferência para URLs já analisadas
CREATE TABLE IF NOT EXISTS llm_cache (
    url                TEXT PRIMARY KEY,
    fluency            DOUBLE PRECISION NOT NULL,
    authoritative_tone DOUBLE PRECISION NOT NULL,
    technical_terms    DOUBLE PRECISION NOT NULL,
    easy_to_understand DOUBLE PRECISION NOT NULL,
    fluency_rec        TEXT,
    auth_rec           TEXT,
    tech_rec           TEXT,
    easy_rec           TEXT,
    cached_at          TEXT NOT NULL
);
