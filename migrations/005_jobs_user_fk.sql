-- Associa jobs a usuários autenticados
ALTER TABLE jobs ADD COLUMN IF NOT EXISTS user_id TEXT REFERENCES users(id);

-- Índice para consultas eficientes por usuário
CREATE INDEX IF NOT EXISTS idx_jobs_user_id ON jobs(user_id);
