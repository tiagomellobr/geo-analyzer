-- Usuários autenticados
CREATE TABLE IF NOT EXISTS users (
    id            TEXT PRIMARY KEY,       -- UUID
    email         TEXT NOT NULL UNIQUE,   -- normalizado: lowercase, trimmed
    password_hash TEXT NOT NULL,           -- Argon2id hash
    created_at    TEXT NOT NULL
);
