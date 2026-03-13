use tower_sessions::Session;

const CSRF_TOKEN_KEY: &str = "csrf_token";

/// Retorna o token CSRF da sessão, criando e armazenando um novo se ainda não existir.
pub async fn get_or_create_csrf_token(session: &Session) -> String {
    if let Ok(Some(token)) = session.get::<String>(CSRF_TOKEN_KEY).await {
        return token;
    }
    let token = uuid::Uuid::new_v4().to_string().replace('-', "");
    let _ = session.insert(CSRF_TOKEN_KEY, &token).await;
    token
}

/// Valida o token CSRF enviado pelo formulário contra o token armazenado na sessão.
/// Retorna `true` apenas se os tokens existirem e forem iguais.
pub async fn validate_csrf_token(session: &Session, submitted: &str) -> bool {
    if submitted.is_empty() {
        return false;
    }
    match session.get::<String>(CSRF_TOKEN_KEY).await {
        Ok(Some(stored)) => constant_time_eq(&stored, submitted),
        _ => false,
    }
}

/// Comparação em tempo constante para prevenir ataques de timing.
fn constant_time_eq(a: &str, b: &str) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.bytes()
        .zip(b.bytes())
        .fold(0u8, |acc, (x, y)| acc | (x ^ y))
        == 0
}
