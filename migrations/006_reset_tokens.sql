-- Tokens de recuperação de senha (uso único, expiração em 1 hora)
CREATE TABLE IF NOT EXISTS password_reset_tokens (
    token      TEXT PRIMARY KEY,       -- UUID v4 gerado aleatoriamente
    user_id    TEXT NOT NULL REFERENCES users(id) ON DELETE CASCADE,
    expires_at TEXT NOT NULL,          -- RFC 3339; app rejeita tokens expirados
    used       INTEGER NOT NULL DEFAULT 0  -- 0 = disponível, 1 = já utilizado
);

CREATE INDEX IF NOT EXISTS idx_reset_tokens_user_id ON password_reset_tokens(user_id);
