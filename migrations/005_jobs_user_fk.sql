-- Associa jobs a usuários autenticados
-- SQLite: ALTER TABLE ADD COLUMN é idempotente com tratamento de erro no código
ALTER TABLE jobs ADD COLUMN user_id TEXT REFERENCES users(id);

-- Índice para consultas eficientes por usuário
CREATE INDEX IF NOT EXISTS idx_jobs_user_id ON jobs(user_id);
