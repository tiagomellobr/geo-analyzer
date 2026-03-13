-- Cache de análise LLM por URL
-- Evita re-executar inferência para URLs já analisadas
CREATE TABLE IF NOT EXISTS llm_cache (
    url                TEXT PRIMARY KEY,
    fluency            REAL NOT NULL,
    authoritative_tone REAL NOT NULL,
    technical_terms    REAL NOT NULL,
    easy_to_understand REAL NOT NULL,
    fluency_rec        TEXT,
    auth_rec           TEXT,
    tech_rec           TEXT,
    easy_rec           TEXT,
    cached_at          TEXT NOT NULL
);
